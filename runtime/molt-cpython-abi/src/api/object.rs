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
    PyObject, PyThreadState, PyTypeObject, PyVectorcallFunc, Py_TPFLAGS_HAVE_VECTORCALL,
};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

type VisitProc = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

/// Molt handle bits for the singleton `None`. Used where an absent bound `self`
/// is correctly represented as Python `None` (e.g. an unbound `PyCFunction`).
/// This is a real, correct value — NOT a fail-open placeholder for a missing
/// result — so it is centralized here rather than materialized inline.
#[inline]
fn none_bits() -> u64 {
    MoltObject::none().bits()
}

// ─── Attribute access ─────────────────────────────────────────────────────

unsafe fn bridge_get_attr_from_name_bits(o: *mut PyObject, name_bits: u64) -> *mut PyObject {
    let obj_bits = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.molt_handle_for_pyobj(o)
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
    // No lookup path resolved the attribute; honor CPython's contract that a
    // failed PyObject_GetAttr sets AttributeError rather than returning a bare
    // NULL that leaves an extension's error check with nothing pending.
    unsafe { attribute_error_missing_obj(o, attr_name) };
    ptr::null_mut()
}

/// Set `AttributeError` for a missing attribute named by a `str` object,
/// mirroring [`attribute_error_missing`] for the `PyObject_GetAttr` path.
/// No-op when an exception is already pending.
unsafe fn attribute_error_missing_obj(o: *mut PyObject, attr_name: *mut PyObject) {
    let attr = unsafe { attr_name_lossy(attr_name) };
    crate::capi_trace::record_silent_failure("PyObject_GetAttr", Some(&attr));
    if exception_already_pending() {
        return;
    }
    let type_name = unsafe { type_name_lossy(o) };
    let message = format!("'{type_name}' object has no attribute '{attr}'");
    if let Ok(cmessage) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_AttributeError,
                cmessage.as_ptr(),
            );
        }
    }
}

/// Best-effort UTF-8 rendering of a `str` attribute-name object for diagnostics.
unsafe fn attr_name_lossy(attr_name: *mut PyObject) -> String {
    if attr_name.is_null() {
        return "?".to_string();
    }
    let mut size: crate::abi_types::Py_ssize_t = 0;
    let ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(attr_name, &mut size) };
    if ptr.is_null() || size < 0 {
        return "?".to_string();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetAttrString(
    o: *mut PyObject,
    attr_name: *const c_char,
) -> *mut PyObject {
    if o.is_null() || attr_name.is_null() {
        return ptr::null_mut();
    }
    let obj_bits = GLOBAL_BRIDGE.lock().molt_handle_for_pyobj(o);
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
    // Attribute not found on any lookup path. CPython's contract is that a
    // failed PyObject_GetAttrString always sets AttributeError; a bare NULL here
    // is a silent failure that leaves an extension's `return -1` with no pending
    // exception. Set the honest exception and record the site for diagnostics.
    unsafe { attribute_error_missing(o, attr_name) };
    ptr::null_mut()
}

/// True when an exception is already pending in either the C-API thread-local
/// store or the runtime's own exception slot. A failing lookup path that finds
/// a pending exception must not overwrite it with a synthetic one — the pending
/// exception is the real error.
fn exception_already_pending() -> bool {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return true;
    }
    unsafe { (hooks_or_stubs().exception_pending)() != 0 }
}

/// Set `AttributeError` for a missing attribute named by a C string, matching
/// CPython's `PyObject_GetAttrString` failure contract, and record the site in
/// the C-API silent-failure tracer. No-op when an exception is already pending.
unsafe fn attribute_error_missing(o: *mut PyObject, attr_name: *const c_char) {
    let attr = unsafe { std::ffi::CStr::from_ptr(attr_name) }.to_string_lossy();
    crate::capi_trace::record_silent_failure("PyObject_GetAttrString", Some(&attr));
    if exception_already_pending() {
        return;
    }
    let type_name = unsafe { type_name_lossy(o) };
    let message = format!("'{type_name}' object has no attribute '{attr}'");
    if let Ok(cmessage) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_AttributeError,
                cmessage.as_ptr(),
            );
        }
    }
}

/// Best-effort type name for an object, for diagnostic messages. Falls back to
/// "object" when the type or its `tp_name` is unavailable.
pub(crate) unsafe fn type_name_lossy(o: *mut PyObject) -> String {
    if o.is_null() {
        return "object".to_string();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return "object".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return "object".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
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
    if !tp.is_null() {
        if let Some(setattro) = unsafe { (*tp).tp_setattro } {
            return unsafe { setattro(o, attr_name, v) };
        }
        // CPython PyObject_SetAttr also tries the legacy `char*` `tp_setattr`
        // slot before giving up.
        if let Some(setattr) = unsafe { (*tp).tp_setattr } {
            let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(attr_name) };
            if name_ptr.is_null() {
                return -1;
            }
            return unsafe { setattr(o, name_ptr, v) };
        }
    }
    // No C set-slot. A bridge-managed Molt object still assigns attributes via
    // the runtime object model (exactly as GenericSetAttr does); route it there.
    // The prior bare -1 silently dropped `setattr` on every slot-less object.
    if GLOBAL_BRIDGE.lock().pyobj_to_handle(o).is_some() {
        return unsafe { PyObject_GenericSetAttr(o, attr_name, v) };
    }
    // Genuinely slot-less foreign type: CPython raises TypeError, never -1.
    unsafe { setattr_no_slot_type_error(o, attr_name, v) };
    -1
}

/// Set the CPython-shaped `TypeError` for `setattr`/`delattr` on an object whose
/// type exposes no assignment slot (`Objects/object.c PyObject_SetAttr`):
/// `'X' object has {no,only read-only} attributes ({assign to,del} .name)`.
/// No-op when an exception is already pending.
unsafe fn setattr_no_slot_type_error(o: *mut PyObject, attr_name: *mut PyObject, v: *mut PyObject) {
    crate::capi_trace::record_silent_failure("PyObject_SetAttr", None);
    if exception_already_pending() {
        return;
    }
    let tp = unsafe { (*o).ob_type };
    let has_read = !tp.is_null()
        && (unsafe { (*tp).tp_getattro }.is_some() || unsafe { (*tp).tp_getattr }.is_some());
    let kind = if has_read {
        "only read-only attributes"
    } else {
        "no attributes"
    };
    let verb = if v.is_null() { "del" } else { "assign to" };
    let type_name = unsafe { type_name_lossy(o) };
    let attr = unsafe { attr_name_lossy(attr_name) };
    let message = format!("'{type_name}' object has {kind} ({verb} .{attr})");
    if let Ok(cmessage) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                cmessage.as_ptr(),
            );
        }
    }
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
    // Legacy `char*` fast path: dispatch `tp_setattr` directly without minting a
    // str (preserved byte-identical for types that install it).
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null()
        && let Some(setattr) = unsafe { (*tp).tp_setattr }
    {
        return unsafe { setattr(o, attr_name, v) };
    }
    // Otherwise build a str key and route through the single `PyObject_SetAttr`
    // authority (CPython `PyObject_SetAttrString`), which dispatches `tp_setattro`,
    // routes bridge objects through `object_set_attr`, and raises the
    // no-attributes `TypeError` — the prior bare -1 dropped the assignment.
    let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(attr_name) };
    if name_obj.is_null() {
        return -1;
    }
    let result = unsafe { PyObject_SetAttr(o, name_obj, v) };
    unsafe { crate::api::refcount::Py_DECREF(name_obj) };
    result
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
        // CPython `_PyObject_LookupAttr` (Objects/object.c): ONLY an
        // `AttributeError` means "attribute absent" (clear + return 0). Any
        // other pending exception — MemoryError / RecursionError / ValueError
        // raised by an optional-dunder getter (numpy `__array__`,
        // `__array_ufunc__`, `__array_interface__`) — MUST propagate as -1 with
        // the exception left set. The prior unconditional `PyErr_Clear` swallowed
        // every one of them as "absent", a silent-wrong-answer on coercion.
        if unsafe {
            crate::api::errors::PyErr_ExceptionMatches(
                &raw mut crate::abi_types::PyExc_AttributeError,
            )
        } != 0
        {
            unsafe { crate::api::errors::PyErr_Clear() };
            return 0;
        }
        // Pathological guard: CPython guarantees a NULL `GetAttr` carries an
        // exception. If a slot broke that contract (NULL with nothing pending),
        // report "absent" rather than emit a -1 sentinel with no exception.
        if !exception_already_pending() {
            return 0;
        }
        return -1;
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
    unsafe { generic_getattr_with_optional_dict(o, name, ptr::null_mut(), 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GenericGetAttrWithDict(
    o: *mut PyObject,
    name: *mut PyObject,
    dict: *mut PyObject,
    suppress: c_int,
) -> *mut PyObject {
    unsafe { generic_getattr_with_optional_dict(o, name, dict, suppress) }
}

unsafe fn descr_get(
    descr: *mut PyObject,
    obj: *mut PyObject,
    owner: *mut PyTypeObject,
) -> Option<*mut PyObject> {
    if descr.is_null() {
        return None;
    }
    let descr_type = unsafe { (*descr).ob_type };
    if descr_type.is_null() {
        return None;
    }
    let get = unsafe { (*descr_type).tp_descr_get }?;
    Some(unsafe { get(descr, obj, owner.cast::<PyObject>()) })
}

unsafe fn generic_getattr_with_optional_dict(
    o: *mut PyObject,
    name: *mut PyObject,
    dict: *mut PyObject,
    suppress: c_int,
) -> *mut PyObject {
    if o.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return ptr::null_mut();
    }
    let descr = unsafe { crate::api::typeobj::_PyType_Lookup(tp, name) };
    if !descr.is_null() && unsafe { crate::api::typeobj::PyDescr_IsData(descr) } != 0 {
        let result = unsafe { descr_get(descr, o, tp).unwrap_or(ptr::null_mut()) };
        if result.is_null() && suppress != 0 {
            unsafe { crate::api::errors::PyErr_Clear() };
        }
        return result;
    }
    // Instance dict: when the caller passed none, load the object's OWN dict from
    // `tp_dictoffset` (CPython `_PyObject_GenericGetAttrWithDict` reads the
    // managed/computed instance dict between the data- and non-data-descriptor
    // tiers). The prior code only consulted an explicitly-passed dict, so every
    // instance attribute was invisible and never shadowed a non-data descriptor.
    // Read-only — no dict is materialized on the attribute-read hot path.
    let mut inst_dict = dict;
    if inst_dict.is_null() {
        let dictptr = unsafe { _PyObject_GetDictPtr(o) };
        if !dictptr.is_null() {
            inst_dict = unsafe { *dictptr };
        }
    }
    if !inst_dict.is_null() {
        let result = unsafe { crate::api::mapping::PyDict_GetItem(inst_dict, name) };
        if !result.is_null() {
            unsafe { crate::api::refcount::Py_INCREF(result) };
            return result;
        }
    }
    if !descr.is_null() {
        if let Some(result) = unsafe { descr_get(descr, o, tp) } {
            if result.is_null() && suppress != 0 {
                unsafe { crate::api::errors::PyErr_Clear() };
            }
            return result;
        }
        unsafe { crate::api::refcount::Py_INCREF(descr) };
        return descr;
    }
    // Nothing resolved. CPython raises `AttributeError: '%.100s' object has no
    // attribute '%U'` unless the caller suppressed it (the `_PyObject_LookupAttr`
    // fast path). The prior bare NULL stranded an `ExceptionMatches(AttributeError)`
    // probe with nothing pending.
    if suppress != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
    } else {
        unsafe { attribute_error_missing_obj(o, name) };
    }
    ptr::null_mut()
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
        // CPython `PyObject_GenericGetDict` raises `AttributeError: This object
        // has no __dict__` rather than returning a bare NULL. (Managed-dict /
        // negative-offset var-objects are not modeled by this ABI tier yet; they
        // take the same honest error until they are.)
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_AttributeError,
                c"This object has no __dict__".as_ptr(),
            );
        }
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
    if o.is_null() {
        return -1;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return -1;
    }
    let offset = unsafe { (*tp).tp_dictoffset };
    if offset <= 0 {
        // CPython raises `AttributeError: This object has no __dict__` rather
        // than a bare -1.
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_AttributeError,
                c"This object has no __dict__".as_ptr(),
            );
        }
        return -1;
    }
    let slot = unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>();
    unsafe {
        // A NULL `value` clears the dict (CPython permits `del obj.__dict__`),
        // rather than the prior blanket -1 rejection.
        if !value.is_null() {
            crate::api::refcount::Py_INCREF(value);
        }
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
pub unsafe extern "C" fn PyObject_ClearWeakRefs(o: *mut PyObject) {
    // CPython `Objects/object.c` walks the weakref list at `tp_weaklistoffset`,
    // clears each referent to NULL and fires its callback. This ABI tier has no
    // weakref *creation* path (`PyWeakref_Check` is always 0 and no
    // `PyWeakReference` is ever installed), so the per-object list head is always
    // NULL — the walk legitimately clears nothing, exactly as CPython does for an
    // object with no live weakrefs. We read the object's ACTUAL list slot rather
    // than ignoring the argument, and if a non-empty list is ever encountered
    // (which the current infrastructure cannot produce) we record it on the
    // permanent silent-failure surface instead of silently mishandling it.
    if o.is_null() {
        return;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return;
    }
    let offset = unsafe { (*tp).tp_weaklistoffset };
    if offset <= 0 {
        // Type declares no weakref support: nothing to clear.
        return;
    }
    let slot = unsafe { (o.cast::<u8>()).offset(offset) }.cast::<*mut PyObject>();
    let head = unsafe { *slot };
    if !head.is_null() {
        // A live weakref list would require the (unmodeled) PyWeakReference
        // layout to clear/fire callbacks; record the site so this is a TRACKED
        // gap rather than a silent no-op, and best-effort empty the slot to match
        // CPython's post-condition (unreachable today).
        crate::capi_trace::record_silent_failure(
            "PyObject_ClearWeakRefs",
            Some(&unsafe { type_name_lossy(o) }),
        );
        unsafe { *slot = ptr::null_mut() };
    }
}

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
        (*obj).ob_refcnt = crate::abi_types::IMMORTAL_REFCNT;
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
pub unsafe extern "C" fn PyThreadState_GetFrame(_tstate: *mut PyThreadState) -> *mut PyFrameObject {
    // CPython's PyThreadState_GetFrame returns a NEW reference to the thread's
    // currently-executing Python frame, or NULL when no frame is on the stack.
    // Molt's cpython-abi PyThreadState carries no CPython-style frame stack
    // (abi_types::PyThreadState has no frame field), so there is no real frame to
    // hand back — the honest, CPython-legal answer is NULL ("no frame currently
    // executing"). The former code fabricated a fresh empty PyFrameObject (NULL
    // code/globals/locals, f_lineno=0) for any non-null tstate: HIDDEN_THEATER
    // (M05) — a C extension walking the frame read fabricated zeros as the real
    // execution frame (and PyFrame_GetCode then synthesized an empty code
    // object). Fail closed with NULL, like the weakref sibling, never a synthetic
    // frame that reads as genuine. `_tstate` is deliberately unused — there is no
    // frame state to consult (not a dropped-arg fail-open).
    ptr::null_mut()
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
    // ── Native bridge-managed Molt object: the runtime object model owns
    // attribute assignment (unchanged fast path). ──
    if let (Some(obj_bits), Some(name_bits)) = (obj_bits, name_bits) {
        let value_bits = value_bits.unwrap_or(0);
        return unsafe { (hooks_or_stubs().object_set_attr)(obj_bits, name_bits, value_bits) };
    }
    // ── Foreign object (bridge miss): CPython `_PyObject_GenericSetAttrWithDict`
    // — a data descriptor's `tp_descr_set` wins, else assign into the instance
    // `__dict__` at `tp_dictoffset`, else an honest `AttributeError`. The prior
    // bare -1 did none of this and left no exception. ──
    unsafe { foreign_generic_setattr(o, name, value) }
}

/// Foreign-object `setattr`, mirroring CPython `_PyObject_GenericSetAttrWithDict`
/// (Objects/object.c): data-descriptor `tp_descr_set` → instance `__dict__`
/// (create-on-assign, delete-on-NULL) → CPython-shaped `AttributeError`.
unsafe fn foreign_generic_setattr(
    o: *mut PyObject,
    name: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return -1;
    }
    // A data descriptor (one whose type defines `tp_descr_set`) takes priority.
    let descr = unsafe { crate::api::typeobj::_PyType_Lookup(tp, name) };
    if !descr.is_null() {
        let dtype = unsafe { (*descr).ob_type };
        if !dtype.is_null()
            && let Some(set) = unsafe { (*dtype).tp_descr_set }
        {
            return unsafe { set(descr, o, value) };
        }
    }
    // Instance `__dict__` at `tp_dictoffset`.
    let dictptr = unsafe { _PyObject_GetDictPtr(o) };
    if !dictptr.is_null() {
        if value.is_null() {
            // Delete: an absent key is an AttributeError (CPython maps the
            // dict's KeyError to AttributeError on the delattr path).
            let d = unsafe { *dictptr };
            if d.is_null() {
                unsafe { attribute_error_missing_obj(o, name) };
                return -1;
            }
            let rc = unsafe { crate::api::mapping::PyDict_DelItem(d, name) };
            if rc < 0
                && unsafe {
                    crate::api::errors::PyErr_ExceptionMatches(
                        &raw mut crate::abi_types::PyExc_KeyError,
                    )
                } != 0
            {
                unsafe {
                    crate::api::errors::PyErr_Clear();
                    attribute_error_missing_obj(o, name);
                }
            }
            return rc;
        }
        let mut d = unsafe { *dictptr };
        if d.is_null() {
            d = unsafe { crate::api::mapping::PyDict_New() };
            if d.is_null() {
                return -1;
            }
            unsafe { *dictptr = d };
        }
        return unsafe { crate::api::mapping::PyDict_SetItem(d, name, value) };
    }
    // No descriptor and no instance dict: CPython raises AttributeError — either
    // "has no attribute" (nothing found) or "attribute is read-only" (a non-data
    // descriptor exists but there is nowhere to store the value).
    crate::capi_trace::record_silent_failure("PyObject_GenericSetAttr", None);
    if !exception_already_pending() {
        let type_name = unsafe { type_name_lossy(o) };
        let attr = unsafe { attr_name_lossy(name) };
        let message = if descr.is_null() {
            format!("'{type_name}' object has no attribute '{attr}'")
        } else {
            format!("'{type_name}' object attribute '{attr}' is read-only")
        };
        if let Ok(cmessage) = std::ffi::CString::new(message) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_AttributeError,
                    cmessage.as_ptr(),
                );
            }
        }
    }
    -1
}

// ─── Truthiness / identity ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsTrue(o: *mut PyObject) -> c_int {
    if o.is_null() {
        return 0;
    }
    // ── Tier 1: singletons — byte-identical zero-overhead fast path. ──
    if std::ptr::eq(o, (&raw mut Py_True).cast::<PyObject>()) {
        return 1;
    }
    if std::ptr::eq(o, (&raw mut Py_False).cast::<PyObject>()) || std::ptr::eq(o, &raw mut Py_None) {
        return 0;
    }
    // ── Tier 2: native Molt object. Resolve the handle once (the lock is dropped
    // immediately so the length hooks below never re-enter a held bridge lock). ──
    let handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(o);
    if let Some(bits) = handle {
        let obj = MoltObject::from_bits(bits);
        // Scalar fast paths — preserved byte-identical.
        if obj.is_none() {
            return 0;
        }
        if obj.is_bool() {
            return obj.as_bool().unwrap_or(false) as c_int;
        }
        if obj.is_int() {
            return (obj.as_int().unwrap_or(0) != 0) as c_int;
        }
        if obj.is_float() {
            return (obj.as_float().unwrap_or(0.0) != 0.0) as c_int;
        }
        // Native container: truthiness is `len != 0` — every empty list / tuple /
        // dict / str / bytes / set is FALSY (CPython `PyObject_IsTrue` via
        // mp_length/sq_length). The prior `else => 1` reported every empty
        // container as truthy (`bool([]) == 1`). Uses the runtime's own length
        // authority — the same primitive the compiled `if x:` consults.
        if obj.is_ptr()
            && let Some(len) = unsafe { native_container_len(bits) }
        {
            return (len != 0) as c_int;
        }
        // A bridge-resolved object with no length notion (a plain Molt object):
        // fall through to type-slot dispatch, then CPython's truthy default.
    }
    // ── Tier 3: foreign / slot dispatch — `nb_bool → mp_length → sq_length`,
    // byte-for-byte as Objects/object.c `PyObject_IsTrue`. The prior `None => 1`
    // never consulted a foreign object's slots (`bool(np.array(...))` was always
    // 1 and never raised for a multi-element array). ──
    unsafe { object_is_true_via_slots(o) }
}

/// Length of a Molt-*native* container handle for truthiness / `len()`, via the
/// runtime's own length hooks (the single length authority). Returns `None` for
/// a handle that is not a native sized container, so callers dispatch type slots.
unsafe fn native_container_len(bits: u64) -> Option<Py_ssize_t> {
    let h = hooks_or_stubs();
    let tag = unsafe { (h.classify_heap)(bits) };
    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
            Some(unsafe { (h.list_len)(bits) } as Py_ssize_t)
        }
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
            Some(unsafe { (h.tuple_len)(bits) } as Py_ssize_t)
        }
        t if t == crate::abi_types::MoltTypeTag::Dict as u8 => {
            Some(unsafe { (h.dict_len)(bits) } as Py_ssize_t)
        }
        t if t == crate::abi_types::MoltTypeTag::Str as u8 => {
            let mut len: usize = 0;
            unsafe { (h.str_data)(bits, &raw mut len) };
            Some(len as Py_ssize_t)
        }
        t if t == crate::abi_types::MoltTypeTag::Bytes as u8 => {
            let mut len: usize = 0;
            unsafe { (h.bytes_data)(bits, &raw mut len) };
            Some(len as Py_ssize_t)
        }
        t if t == crate::abi_types::MoltTypeTag::Set as u8 => {
            let n = unsafe { (h.set_size)(bits) };
            if n >= 0 { Some(n as Py_ssize_t) } else { None }
        }
        _ => None,
    }
}

/// Foreign / slot-based truthiness: dispatch `nb_bool → mp_length → sq_length`
/// and default to truthy, byte-for-byte as `Objects/object.c PyObject_IsTrue`.
/// Reads the type's slot structs directly (no bridge lock) so the always-on hot
/// path stays lean. Returns 0/1, or -1 with the slot's pending exception.
unsafe fn object_is_true_via_slots(o: *mut PyObject) -> c_int {
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return 1;
    }
    // `nb_bool` (inquiry: `int (*)(PyObject*)`).
    let num = unsafe { (*tp).tp_as_number }.cast::<crate::abi_types::PyNumberMethods>();
    if !num.is_null() {
        let slot = unsafe { (*num).nb_bool };
        if !slot.is_null() {
            type Inquiry = unsafe extern "C" fn(*mut PyObject) -> c_int;
            let f: Inquiry = unsafe { std::mem::transmute::<*mut c_void, Inquiry>(slot) };
            let res = unsafe { f(o) };
            return if res > 0 { 1 } else { res };
        }
    }
    // `mp_length`, then `sq_length` (lenfunc: `Py_ssize_t (*)(PyObject*)`).
    let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
    if !m.is_null() {
        let slot = unsafe { (*m).mp_length };
        if !slot.is_null() {
            return unsafe { len_slot_truthiness(o, slot) };
        }
    }
    let seq = unsafe { (*tp).tp_as_sequence }.cast::<crate::abi_types::PySequenceMethods>();
    if !seq.is_null() {
        let slot = unsafe { (*seq).sq_length };
        if !slot.is_null() {
            return unsafe { len_slot_truthiness(o, slot) };
        }
    }
    // No `nb_bool`/`mp_length`/`sq_length`: CPython returns 1 (truthy default).
    1
}

/// Call a `lenfunc` slot and fold its result to a truthiness `c_int`, exactly as
/// CPython's `return (res > 0) ? 1 : Py_SAFE_DOWNCAST(res, Py_ssize_t, int);`
/// (a negative `res` is the slot's error sentinel and is propagated).
unsafe fn len_slot_truthiness(o: *mut PyObject, slot: *mut c_void) -> c_int {
    type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
    let f: LenFunc = unsafe { std::mem::transmute::<*mut c_void, LenFunc>(slot) };
    let res = unsafe { f(o) };
    if res > 0 { 1 } else { res as c_int }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Print(
    o: *mut PyObject,
    fp: *mut libc::FILE,
    flags: c_int,
) -> c_int {
    if fp.is_null() {
        return -1;
    }
    // CPython prints "<nil>" for a NULL object and returns 0.
    if o.is_null() {
        let rc = unsafe { libc::fputs(c"<nil>".as_ptr(), fp) };
        return if rc < 0 { -1 } else { 0 };
    }
    // CPython `Objects/object.c PyObject_Print` selects `repr()` by default and
    // `str()` only when `Py_PRINT_RAW` (==1) is set; the prior code always used
    // `str`, so `PyObject_Print(o, fp, 0)` printed str(o) where CPython prints
    // repr(o).
    const PY_PRINT_RAW: c_int = 1;
    let rendered = if flags & PY_PRINT_RAW != 0 {
        unsafe { crate::api::typeobj::PyObject_Str(o) }
    } else {
        unsafe { crate::api::typeobj::PyObject_Repr(o) }
    };
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

    // Foreign object (bridge miss): CPython `PyObject_Format` dispatches the
    // object's OWN `__format__` (`_PyObject_LookupSpecial`) for every object;
    // only `object.__format__` rejects a non-empty spec. This reaches a foreign
    // type's `__format__` (numpy scalar/dtype string formatting) that the prior
    // blanket TypeError skipped. Bridge objects already went through the runtime
    // `object_format` hook above.
    if obj_bits.is_none()
        && let Some(out) = unsafe { foreign_dispatch_format(o, spec) }
    {
        if !owned_empty_spec.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(owned_empty_spec) };
        }
        return out;
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

/// Dispatch a foreign object's own `__format__(spec)` (CPython
/// `_PyObject_LookupSpecial` + call). Returns `Some(result)` when the object
/// defines `__format__` (the result may be NULL with a pending exception from
/// the call), or `None` when it has no `__format__` so the caller falls back.
unsafe fn foreign_dispatch_format(o: *mut PyObject, spec: *mut PyObject) -> Option<*mut PyObject> {
    let meth = unsafe { PyObject_GetAttrString(o, c"__format__".as_ptr()) };
    if meth.is_null() {
        // No `__format__`: clear the AttributeError and let the caller fall back.
        unsafe { crate::api::errors::PyErr_Clear() };
        return None;
    }
    let result = unsafe { PyObject_CallOneArg(meth, spec) };
    unsafe { crate::api::refcount::Py_DECREF(meth) };
    Some(result)
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
    // ── Native Molt container fast path (list/tuple/dict/str/bytes/set) via the
    // single length authority. `set` was previously unhandled (silent -1). ──
    let handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(o);
    if let Some(bits) = handle {
        let obj = MoltObject::from_bits(bits);
        if obj.is_ptr()
            && let Some(len) = unsafe { native_container_len(bits) }
        {
            return len;
        }
    }
    // ── Foreign fallback: CPython `PyObject_Size` dispatches `sq_length`, then
    // `mp_length` (via `PyMapping_Size`), else raises `TypeError: object of type
    // 'X' has no len()`. The prior bare -1 consulted neither slot and left no
    // exception (`len(np.ndarray)` was a silent -1). ──
    unsafe { foreign_object_size(o) }
}

/// CPython `PyObject_Size` foreign path: `sq_length` then `mp_length`, else a
/// `TypeError: object of type '%.200s' has no len()`. Every -1 carries an
/// exception (the slot's own, or the `len()` TypeError). Reads the type's slots
/// directly (no bridge lock).
unsafe fn foreign_object_size(o: *mut PyObject) -> Py_ssize_t {
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null() {
        let seq = unsafe { (*tp).tp_as_sequence }.cast::<crate::abi_types::PySequenceMethods>();
        if !seq.is_null() {
            let slot = unsafe { (*seq).sq_length };
            if !slot.is_null() {
                type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
                let f: LenFunc = unsafe { std::mem::transmute::<*mut c_void, LenFunc>(slot) };
                return unsafe { f(o) };
            }
        }
        let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
        if !m.is_null() {
            let slot = unsafe { (*m).mp_length };
            if !slot.is_null() {
                type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
                let f: LenFunc = unsafe { std::mem::transmute::<*mut c_void, LenFunc>(slot) };
                return unsafe { f(o) };
            }
        }
    }
    let message = format!("object of type '{}' has no len()", unsafe {
        type_name_lossy(o)
    });
    let _ = unsafe { item_type_error_obj(o, "PyObject_Size", message) };
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

/// Enforce the C-API contract that an object-returning item-access slot which
/// returns NULL leaves a pending exception. If a foreign slot broke that
/// contract, record the site on the permanent silent-failure surface
/// (`MOLT_TRACE_CAPI`) and raise `SystemError` so a bare NULL never escapes.
/// Mirrors `abstract_number::finalize_slot_result`.
unsafe fn finalize_item_slot_result(
    o: *mut PyObject,
    result: *mut PyObject,
    capi_name: &str,
) -> *mut PyObject {
    if result.is_null() && !exception_already_pending() {
        crate::capi_trace::record_silent_failure(capi_name, Some(&unsafe { type_name_lossy(o) }));
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"item-access slot returned NULL without setting an exception".as_ptr(),
            );
        }
    }
    result
}

/// `int`-returning analogue of [`finalize_item_slot_result`] for the
/// item-assignment slots (`mp_ass_subscript` / `sq_ass_item`): a `-1` return
/// must carry a pending exception, else record + raise `SystemError`.
unsafe fn finalize_item_ass_result(o: *mut PyObject, res: c_int, capi_name: &str) -> c_int {
    if res < 0 && !exception_already_pending() {
        crate::capi_trace::record_silent_failure(capi_name, Some(&unsafe { type_name_lossy(o) }));
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"item-assignment slot returned -1 without setting an exception".as_ptr(),
            );
        }
    }
    res
}

/// No native or foreign item access applied: record the site on the permanent
/// silent-failure surface and raise a CPython-shaped `TypeError` (`message`), so
/// a failing subscript is never a bare NULL. Object-returning variant.
unsafe fn item_type_error_obj(o: *mut PyObject, capi_name: &str, message: String) -> *mut PyObject {
    crate::capi_trace::record_silent_failure(capi_name, Some(&unsafe { type_name_lossy(o) }));
    if !exception_already_pending()
        && let Ok(cmsg) = std::ffi::CString::new(message)
    {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                cmsg.as_ptr(),
            );
        }
    }
    ptr::null_mut()
}

/// `int`-returning analogue of [`item_type_error_obj`] (returns the `-1`
/// sentinel for the assignment protocol).
unsafe fn item_type_error_int(o: *mut PyObject, capi_name: &str, message: String) -> c_int {
    let _ = unsafe { item_type_error_obj(o, capi_name, message) };
    -1
}

/// Convert an index-like `key` to a `Py_ssize_t` for the sequence protocol,
/// exactly as CPython's `PyObject_GetItem`/`PyObject_SetItem` sequence path
/// (`_PyIndex_Check` → `PyNumber_AsSsize_t(key, PyExc_IndexError)`), then apply
/// the negative-index adjustment `PySequence_GetItem`/`SetItem` performs via the
/// object's own `sq_length`. Returns `Ok(index)`, or `Err(())` when the key is
/// not index-like or the conversion raised (a pending exception is set, or the
/// caller must raise the "sequence index must be integer" `TypeError`).
unsafe fn sequence_index_from_key(
    o: *mut PyObject,
    key: *mut PyObject,
    seq: *mut crate::abi_types::PySequenceMethods,
) -> Result<Py_ssize_t, ()> {
    if unsafe { crate::api::abstract_number::PyIndex_Check(key) } == 0 {
        return Err(());
    }
    let mut idx = unsafe {
        crate::api::abstract_number::PyNumber_AsSsize_t(
            key,
            &raw mut crate::abi_types::PyExc_IndexError,
        )
    };
    if idx == -1 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return Err(());
    }
    // Negative-index adjustment via sq_length (CPython PySequence_GetItem/SetItem).
    if idx < 0 {
        let sq_length = unsafe { (*seq).sq_length };
        if !sq_length.is_null() {
            type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
            let lf: LenFunc = unsafe { std::mem::transmute::<*mut c_void, LenFunc>(sq_length) };
            let l = unsafe { lf(o) };
            if l < 0 {
                // sq_length raised — propagate its exception, not a synthetic one.
                return Err(());
            }
            idx += l;
        }
    }
    Ok(idx)
}

/// Foreign (non-native) item read: dispatch the object's type slots in CPython's
/// order — `tp_as_mapping->mp_subscript`, then `tp_as_sequence->sq_item` (with
/// `__index__` conversion + negative-index adjustment via `sq_length`) — exactly
/// as `Objects/abstract.c` `PyObject_GetItem`. Returns NULL only with a pending
/// exception set. This is the FALLBACK tier below the Molt-native dict/list/tuple
/// fast paths, mirroring how `abstract_number::call_number_unary_slot` layers the
/// foreign number-slot dispatch below the native numeric fast path.
unsafe fn foreign_get_item(o: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        let message = format!("'{}' object is not subscriptable", unsafe { type_name_lossy(o) });
        return unsafe { item_type_error_obj(o, "PyObject_GetItem", message) };
    }
    // Mapping protocol first: mp_subscript(o, key).
    let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
    if !m.is_null() {
        let mp_subscript = unsafe { (*m).mp_subscript };
        if !mp_subscript.is_null() {
            type BinaryFunc = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
            let f: BinaryFunc =
                unsafe { std::mem::transmute::<*mut c_void, BinaryFunc>(mp_subscript) };
            let result = unsafe { f(o, key) };
            return unsafe { finalize_item_slot_result(o, result, "PyObject_GetItem") };
        }
    }
    // Sequence protocol: sq_item(o, index).
    let seq = unsafe { (*tp).tp_as_sequence }.cast::<crate::abi_types::PySequenceMethods>();
    if !seq.is_null() {
        let sq_item = unsafe { (*seq).sq_item };
        if !sq_item.is_null() {
            let idx = match unsafe { sequence_index_from_key(o, key, seq) } {
                Ok(i) => i,
                Err(()) => {
                    // A pending exception (conversion / sq_length) is the real
                    // error; otherwise the key is not index-like.
                    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        return ptr::null_mut();
                    }
                    let message = format!(
                        "sequence index must be integer, not '{}'",
                        unsafe { type_name_lossy(key) }
                    );
                    return unsafe { item_type_error_obj(o, "PyObject_GetItem", message) };
                }
            };
            type SsizeArgFunc = unsafe extern "C" fn(*mut PyObject, Py_ssize_t) -> *mut PyObject;
            let f: SsizeArgFunc = unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(sq_item) };
            let result = unsafe { f(o, idx) };
            return unsafe { finalize_item_slot_result(o, result, "PyObject_GetItem") };
        }
    }
    // No mapping or sequence protocol: honest TypeError (CPython
    // "'%.200s' object is not subscriptable").
    let message = format!("'{}' object is not subscriptable", unsafe { type_name_lossy(o) });
    unsafe { item_type_error_obj(o, "PyObject_GetItem", message) }
}

/// Foreign (non-native) item write: dispatch the object's type slots in
/// CPython's order — `tp_as_mapping->mp_ass_subscript`, then
/// `tp_as_sequence->sq_ass_item` (index conversion + negative adjustment) — as
/// `Objects/abstract.c` `PyObject_SetItem`. Returns `-1` only with a pending
/// exception set. FALLBACK tier below the Molt-native dict lane.
unsafe fn foreign_set_item(o: *mut PyObject, key: *mut PyObject, v: *mut PyObject) -> c_int {
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        let message = format!(
            "'{}' object does not support item assignment",
            unsafe { type_name_lossy(o) }
        );
        return unsafe { item_type_error_int(o, "PyObject_SetItem", message) };
    }
    // Mapping protocol first: mp_ass_subscript(o, key, v).
    let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
    if !m.is_null() {
        let mp_ass = unsafe { (*m).mp_ass_subscript };
        if !mp_ass.is_null() {
            type ObjObjArgProc =
                unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int;
            let f: ObjObjArgProc = unsafe { std::mem::transmute::<*mut c_void, ObjObjArgProc>(mp_ass) };
            let res = unsafe { f(o, key, v) };
            return unsafe { finalize_item_ass_result(o, res, "PyObject_SetItem") };
        }
    }
    // Sequence protocol: sq_ass_item(o, index, v).
    let seq = unsafe { (*tp).tp_as_sequence }.cast::<crate::abi_types::PySequenceMethods>();
    if !seq.is_null() {
        let sq_ass = unsafe { (*seq).sq_ass_item };
        if !sq_ass.is_null() {
            let idx = match unsafe { sequence_index_from_key(o, key, seq) } {
                Ok(i) => i,
                Err(()) => {
                    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        return -1;
                    }
                    let message = format!(
                        "sequence index must be integer, not '{}'",
                        unsafe { type_name_lossy(key) }
                    );
                    return unsafe { item_type_error_int(o, "PyObject_SetItem", message) };
                }
            };
            type SsizeObjArgProc =
                unsafe extern "C" fn(*mut PyObject, Py_ssize_t, *mut PyObject) -> c_int;
            let f: SsizeObjArgProc = unsafe { std::mem::transmute::<*mut c_void, SsizeObjArgProc>(sq_ass) };
            let res = unsafe { f(o, idx, v) };
            return unsafe { finalize_item_ass_result(o, res, "PyObject_SetItem") };
        }
    }
    // No mapping or sequence assignment protocol: honest TypeError.
    let message = format!(
        "'{}' object does not support item assignment",
        unsafe { type_name_lossy(o) }
    );
    unsafe { item_type_error_int(o, "PyObject_SetItem", message) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetItem(o: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    if o.is_null() || key.is_null() {
        return ptr::null_mut();
    }
    // ── Molt-native fast path (dict/list/tuple lanes — ordering unchanged) ──
    // A bridge miss (`None`) means `o` is a genuine foreign C-extension object
    // that never crossed into Molt: fall through to the foreign-slot dispatch
    // tier instead of the prior bare-NULL return.
    let o_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(o);
    if let Some(o_bits) = o_bits {
        let h = hooks_or_stubs();
        let obj = MoltObject::from_bits(o_bits);
        if obj.is_ptr() {
            let tag = unsafe { (h.classify_heap)(o_bits) };
            // Dict: use dict_get
            if tag == crate::abi_types::MoltTypeTag::Dict as u8 {
                let key_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(key);
                if let Some(key_bits) = key_bits {
                    let val_bits = unsafe { (h.dict_get)(o_bits, key_bits) };
                    if val_bits == 0 {
                        // Dict miss: CPython `dict_subscript` raises `KeyError`
                        // with the key as its argument (Objects/dictobject.c).
                        // The prior bare NULL stranded any C caller relying on
                        // the set-exception contract.
                        unsafe {
                            crate::api::errors::PyErr_SetObject(
                                &raw mut crate::abi_types::PyExc_KeyError,
                                key,
                            );
                        }
                        return ptr::null_mut();
                    }
                    return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(val_bits) };
                }
                // Foreign key into a native dict: fall to the generic slot path.
            }
            // List: use list_item with int key
            if tag == crate::abi_types::MoltTypeTag::List as u8 {
                let key_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(key);
                if let Some(key_bits) = key_bits {
                    let key_obj = MoltObject::from_bits(key_bits);
                    if let Some(idx) = key_obj.as_int() {
                        let len = unsafe { (h.list_len)(o_bits) };
                        let actual_idx = if idx < 0 { len as i64 + idx } else { idx };
                        if actual_idx < 0 || actual_idx >= len as i64 {
                            // CPython list indexing raises IndexError, not NULL.
                            unsafe {
                                crate::api::errors::PyErr_SetString(
                                    &raw mut crate::abi_types::PyExc_IndexError,
                                    c"list index out of range".as_ptr(),
                                );
                            }
                            return ptr::null_mut();
                        }
                        let item_bits = unsafe { (h.list_item)(o_bits, actual_idx as usize) };
                        if item_bits == 0 {
                            return ptr::null_mut();
                        }
                        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) };
                    }
                }
            }
            // Tuple: use tuple_item with int key
            if tag == crate::abi_types::MoltTypeTag::Tuple as u8 {
                let key_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(key);
                if let Some(key_bits) = key_bits {
                    let key_obj = MoltObject::from_bits(key_bits);
                    if let Some(idx) = key_obj.as_int() {
                        let len = unsafe { (h.tuple_len)(o_bits) };
                        let actual_idx = if idx < 0 { len as i64 + idx } else { idx };
                        if actual_idx < 0 || actual_idx >= len as i64 {
                            unsafe {
                                crate::api::errors::PyErr_SetString(
                                    &raw mut crate::abi_types::PyExc_IndexError,
                                    c"tuple index out of range".as_ptr(),
                                );
                            }
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
        }
    }
    // ── Foreign fallback tier: dispatch mp_subscript → sq_item, else TypeError.
    unsafe { foreign_get_item(o, key) }
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
    // ── Molt-native fast path (dict lane — ordering unchanged). A bridge miss
    // means `o` is a genuine foreign object; fall through to slot dispatch. ──
    let o_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(o);
    if let Some(o_bits) = o_bits {
        let h = hooks_or_stubs();
        let obj = MoltObject::from_bits(o_bits);
        if obj.is_ptr() {
            let tag = unsafe { (h.classify_heap)(o_bits) };
            if tag == crate::abi_types::MoltTypeTag::Dict as u8 {
                let bridge2 = GLOBAL_BRIDGE.lock();
                let key_bits = bridge2.pyobj_to_handle(key);
                let val_bits = bridge2.pyobj_to_handle(v);
                drop(bridge2);
                if let (Some(key_bits), Some(val_bits)) = (key_bits, val_bits) {
                    unsafe { (h.dict_set)(o_bits, key_bits, val_bits) };
                    return 0;
                }
                // Foreign key/value into a native dict: fall to the slot path.
            }
        }
    }
    // ── Foreign fallback tier: mp_ass_subscript → sq_ass_item, else TypeError.
    unsafe { foreign_set_item(o, key, v) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_DelItem(o: *mut PyObject, key: *mut PyObject) -> c_int {
    if o.is_null() || key.is_null() {
        return -1;
    }
    // ── Native dict fast path: real deletion via the runtime dict authority.
    // A bridge miss means `o` is a genuine foreign object; fall to slot dispatch.
    let o_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(o);
    if let Some(o_bits) = o_bits {
        let h = hooks_or_stubs();
        let obj = MoltObject::from_bits(o_bits);
        if obj.is_ptr() {
            let tag = unsafe { (h.classify_heap)(o_bits) };
            if tag == crate::abi_types::MoltTypeTag::Dict as u8 {
                let key_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(key);
                if let Some(key_bits) = key_bits {
                    // dict_del removes the entry and returns -1 (KeyError set by
                    // the runtime) if absent.
                    return unsafe { (h.dict_del)(o_bits, key_bits) };
                }
                // Foreign key into a native dict: fall to the slot path.
            }
        }
    }
    // ── Foreign fallback: dispatch `mp_ass_subscript(o, key, NULL)` then
    // `sq_ass_item(o, idx, NULL)`, else an honest TypeError — CPython
    // `PyObject_DelItem`. The prior bare -1 never dispatched a deletion slot
    // (`del seq[i]` on a foreign object silently failed). ──
    unsafe { foreign_del_item(o, key) }
}

/// Foreign item deletion: `mp_ass_subscript(o, key, NULL)` then
/// `sq_ass_item(o, idx, NULL)`, else `TypeError: '%.200s' object doesn't support
/// item deletion` (CPython `Objects/abstract.c PyObject_DelItem`). Returns -1
/// only with a pending exception set.
unsafe fn foreign_del_item(o: *mut PyObject, key: *mut PyObject) -> c_int {
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null() {
        let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
        if !m.is_null() {
            let mp_ass = unsafe { (*m).mp_ass_subscript };
            if !mp_ass.is_null() {
                type ObjObjArgProc =
                    unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int;
                let f: ObjObjArgProc =
                    unsafe { std::mem::transmute::<*mut c_void, ObjObjArgProc>(mp_ass) };
                let res = unsafe { f(o, key, ptr::null_mut()) };
                return unsafe { finalize_item_ass_result(o, res, "PyObject_DelItem") };
            }
        }
        let seq = unsafe { (*tp).tp_as_sequence }.cast::<crate::abi_types::PySequenceMethods>();
        if !seq.is_null() {
            let sq_ass = unsafe { (*seq).sq_ass_item };
            if !sq_ass.is_null() {
                let idx = match unsafe { sequence_index_from_key(o, key, seq) } {
                    Ok(i) => i,
                    Err(()) => {
                        if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                            return -1;
                        }
                        let message = format!("sequence index must be integer, not '{}'", unsafe {
                            type_name_lossy(key)
                        });
                        return unsafe { item_type_error_int(o, "PyObject_DelItem", message) };
                    }
                };
                type SsizeObjArgProc =
                    unsafe extern "C" fn(*mut PyObject, Py_ssize_t, *mut PyObject) -> c_int;
                let f: SsizeObjArgProc =
                    unsafe { std::mem::transmute::<*mut c_void, SsizeObjArgProc>(sq_ass) };
                let res = unsafe { f(o, idx, ptr::null_mut()) };
                return unsafe { finalize_item_ass_result(o, res, "PyObject_DelItem") };
            }
        }
    }
    let message = format!("'{}' object doesn't support item deletion", unsafe {
        type_name_lossy(o)
    });
    unsafe { item_type_error_int(o, "PyObject_DelItem", message) }
}

// ─── Iterator protocol ───────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetIter(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return ptr::null_mut();
    }
    if let Some(iter_fn) = unsafe { (*tp).tp_iter } {
        // Validate the result is itself an iterator (CPython `Objects/abstract.c
        // PyObject_GetIter`: "iter() returned non-iterator of type '%.100s'").
        let res = unsafe { iter_fn(o) };
        if !res.is_null() && unsafe { PyIter_Check(res) } == 0 {
            let message = format!(
                "iter() returned non-iterator of type '{}'",
                unsafe { type_name_lossy(res) }
            );
            unsafe { crate::api::refcount::Py_DECREF(res) };
            if !exception_already_pending()
                && let Ok(cmsg) = std::ffi::CString::new(message)
            {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_TypeError,
                        cmsg.as_ptr(),
                    );
                }
            }
            return ptr::null_mut();
        }
        return res;
    }
    // No `tp_iter`: CPython falls back to an index-based sequence iterator for any
    // object supporting the sequence protocol (`sq_item`), else raises
    // `TypeError: '%.200s' object is not iterable`. The prior bare NULL returned
    // no iterator and set no exception.
    if unsafe { crate::api::abstract_sequence::PySequence_Check(o) } != 0 {
        return unsafe { PySeqIter_New(o) };
    }
    let message = format!("'{}' object is not iterable", unsafe { type_name_lossy(o) });
    let _ = unsafe { item_type_error_obj(o, "PyObject_GetIter", message) };
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
        let result = unsafe { iternext(iter) };
        if result.is_null()
            && unsafe {
                crate::api::errors::PyErr_ExceptionMatches(
                    &raw mut crate::abi_types::PyExc_StopIteration,
                )
            } != 0
        {
            // CPython `Objects/abstract.c PyIter_Next` clears a normal
            // end-of-iteration StopIteration so a NULL result carries no
            // exception; the prior code left it pending for the caller.
            unsafe { crate::api::errors::PyErr_Clear() };
        }
        return result;
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

/// Index-based sequence-iterator instance (CPython `Objects/iterobject.c`
/// `seqiterobject`): iterates `it_seq` via `PySequence_GetItem(it_seq, it_index)`
/// regardless of whether the sequence defines `tp_iter`.
#[repr(C)]
struct MoltSeqIterObject {
    ob_base: PyObject,
    /// Strong reference to the underlying sequence; NULL once exhausted.
    it_seq: *mut PyObject,
    it_index: Py_ssize_t,
}

static mut MOLT_SEQITER_TYPE: PyTypeObject = unsafe { std::mem::zeroed() };
static MOLT_SEQITER_TYPE_INIT: std::sync::Once = std::sync::Once::new();

/// Lazily initialize and return the shared sequence-iterator type object. Only
/// the `tp_iter`/`tp_iternext`/`tp_dealloc` slots are load-bearing; the type is
/// consumed solely by `PyIter_Next` (which reads `tp_iternext`).
unsafe fn molt_seqiter_type() -> *mut PyTypeObject {
    MOLT_SEQITER_TYPE_INIT.call_once(|| {
        let t = &raw mut MOLT_SEQITER_TYPE;
        unsafe {
            (*t).ob_base.ob_base.ob_refcnt = crate::abi_types::IMMORTAL_REFCNT;
            (*t).ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
            (*t).tp_name = c"iterator".as_ptr();
            (*t).tp_basicsize = std::mem::size_of::<MoltSeqIterObject>() as Py_ssize_t;
            (*t).tp_iter = Some(molt_seqiter_self);
            (*t).tp_iternext = Some(molt_seqiter_next);
            (*t).tp_dealloc = Some(molt_seqiter_dealloc);
        }
    });
    &raw mut MOLT_SEQITER_TYPE
}

unsafe extern "C" fn molt_seqiter_self(it: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::refcount::Py_INCREF(it) };
    it
}

unsafe extern "C" fn molt_seqiter_next(it: *mut PyObject) -> *mut PyObject {
    let iter = it.cast::<MoltSeqIterObject>();
    let seq = unsafe { (*iter).it_seq };
    if seq.is_null() {
        return ptr::null_mut(); // already exhausted
    }
    let idx = unsafe { (*iter).it_index };
    let item = unsafe { crate::api::abstract_sequence::PySequence_GetItem(seq, idx) };
    if !item.is_null() {
        unsafe { (*iter).it_index = idx + 1 };
        return item;
    }
    // End-of-iteration (IndexError/StopIteration) is cleared and drops the
    // sequence ref, exactly as CPython `iter_iternext`; any other exception
    // propagates.
    if unsafe {
        crate::api::errors::PyErr_ExceptionMatches(&raw mut crate::abi_types::PyExc_IndexError)
    } != 0
        || unsafe {
            crate::api::errors::PyErr_ExceptionMatches(
                &raw mut crate::abi_types::PyExc_StopIteration,
            )
        } != 0
    {
        unsafe {
            crate::api::errors::PyErr_Clear();
            (*iter).it_seq = ptr::null_mut();
            crate::api::refcount::Py_DECREF(seq);
        }
    }
    ptr::null_mut()
}

unsafe extern "C" fn molt_seqiter_dealloc(it: *mut PyObject) {
    let iter = it.cast::<MoltSeqIterObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*iter).it_seq);
        drop(Box::from_raw(iter));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySeqIter_New(seq: *mut PyObject) -> *mut PyObject {
    // CPython `PySeqIter_New` raises on a NULL argument and otherwise builds a
    // real index-based iterator (NOT `PyObject_GetIter`, which would dispatch
    // `tp_iter` and return the prior silent NULL for a `tp_iter`-less sequence).
    if seq.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let ty = unsafe { molt_seqiter_type() };
    let obj = Box::new(MoltSeqIterObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: ty,
        },
        it_seq: seq,
        it_index: 0,
    });
    unsafe { crate::api::refcount::Py_INCREF(seq) };
    Box::into_raw(obj).cast::<PyObject>()
}

// ─── Dir ──────────────────────────────────────────────────────────────────

/// `PyObject_Dir(o)` — return `dir(o)` as a sorted list (Objects/object.c).
///
/// Routes to the runtime dir authority (`object_dir` hook: MRO walk, `__dict__`,
/// `__dir__`). The previous stub returned an empty list ignoring `o`, so every
/// extension `PyObject_Dir(obj)` came back empty (a silent-wrong-answer fail-open).
///
/// `PyObject_Dir(NULL)` in CPython returns the *caller's* local names, which is a
/// frame-introspection operation the ABI layer has no frame for; we fail closed
/// with a precise SystemError rather than fabricating an answer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Dir(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyObject_Dir(NULL) (frame-local dir) is not supported from the C-API bridge"
                    .as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let o_handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(o);
    let bits = match o_handle {
        Some(b) => b,
        None => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"PyObject_Dir: argument is not a bridge-managed object".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
    };
    let h = hooks_or_stubs();
    let result = unsafe { (h.object_dir)(bits) };
    if result == 0 {
        // Runtime set a pending exception, or hooks are unregistered. Guarantee a
        // NULL return carries an exception (ABI contract).
        let pending = crate::hooks::hooks()
            .map(|h| unsafe { (h.exception_pending)() } != 0)
            .unwrap_or(false);
        if !pending {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"PyObject_Dir failed: runtime dir authority unavailable".as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result) }
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
    // Vectorcall-first (CPython 3.12 `_PyObject_Call`, Objects/call.c): if the
    // callable advertises a `vectorcallfunc`, invoke it via the PEP-590
    // `_PyVectorcall_Call` conversion (tuple/dict → flat stack) rather than
    // `tp_call`. This is what makes the documented `tp_call = PyVectorcall_Call`
    // pattern TERMINATE: the object's own slot is read directly here, so we never
    // fall into `tp_call → PyVectorcall_Call → PyObject_Call → tp_call → …`.
    if let Some(func) = unsafe { vectorcall_function(callable) } {
        return unsafe { vectorcall_call_with_tuple(func, callable, args, kwargs) };
    }
    let tp = unsafe { (*callable).ob_type };
    if !tp.is_null()
        && let Some(call) = unsafe { (*tp).tp_call }
    {
        return unsafe { call(callable, args, kwargs) };
    }
    // Bridge-managed Molt callable (a compiled function / class / bound method
    // handed back by `PyObject_GetAttrString` &c. — e.g. numpy calling
    // `numpy.dtypes._add_dtype_helper`): bridge proxies carry no `tp_call`, so
    // route through the runtime's single call authority (`object_call` hook:
    // dispatch, kwargs binding, CPython-shaped exceptions). Raw-registered C
    // objects are excluded — their synthetic handles are identity anchors, not
    // Molt object bits — and fall through to the honest TypeError below.
    let callable_bits = GLOBAL_BRIDGE.lock().molt_handle_for_pyobj(callable);
    if let Some(callable_bits) = callable_bits {
        return unsafe { call_bridged_callable(callable_bits, args, kwargs) };
    }
    // No tp_call slot: the object is not callable through this path. CPython
    // raises TypeError here; a bare NULL is a silent failure that strands an
    // extension's error check with no pending exception.
    let type_name = unsafe { type_name_lossy(callable) };
    crate::capi_trace::record_silent_failure("PyObject_Call", Some(&type_name));
    if !exception_already_pending() {
        let message = format!("'{type_name}' object is not callable");
        if let Ok(cmessage) = std::ffi::CString::new(message) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_TypeError,
                    cmessage.as_ptr(),
                );
            }
        }
    }
    ptr::null_mut()
}

/// Invoke a Molt callable handle through the runtime `object_call` hook,
/// translating the C-API `(args, kwargs)` pair to Molt handles. `args` /
/// `kwargs` may be NULL (and `kwargs` may be `Py_None`), per the
/// `PyObject_Call` contract. Fails loudly (TypeError/SystemError, never a bare
/// NULL) when a piece cannot be resolved — a silently dropped call argument is
/// a wrong answer, not a fallback.
unsafe fn call_bridged_callable(
    callable_bits: u64,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    // Set when the args tuple was marshaled from a C-layout tuple: the fresh
    // Molt tuple is owned here and released after the call.
    let mut owned_args_bits: Option<u64> = None;
    let args_bits = if args.is_null() {
        0
    } else {
        let resolved = GLOBAL_BRIDGE.lock().molt_handle_for_pyobj(args);
        match resolved {
            Some(bits) => bits,
            // The shim's `PyTuple_New` / `PyTuple_Pack` mint C-LAYOUT tuples
            // (`PyTupleObject`, not bridge proxies) — the normal shape for
            // `PyObject_CallFunction`-built args. Marshal the C tuple's items
            // into a fresh Molt tuple; each item resolves by identity
            // (bridge proxies, singletons, or raw-registered opaque tokens).
            None => match unsafe { molt_tuple_bits_from_c_tuple(args) } {
                Some(bits) => {
                    owned_args_bits = Some(bits);
                    bits
                }
                None => {
                    crate::capi_trace::record_silent_failure(
                        "PyObject_Call",
                        Some("unresolved args tuple"),
                    );
                    if !exception_already_pending() {
                        unsafe {
                            crate::api::errors::PyErr_SetString(
                                &raw mut crate::abi_types::PyExc_SystemError,
                                c"PyObject_Call: args tuple is not a bridge-managed object and has no C tuple layout"
                                    .as_ptr(),
                            );
                        }
                    }
                    return ptr::null_mut();
                }
            },
        }
    };
    let kwargs_bits = if kwargs.is_null()
        || std::ptr::eq(kwargs, &raw mut crate::abi_types::Py_None)
    {
        0
    } else {
        let kwargs_handle = GLOBAL_BRIDGE.lock().molt_handle_for_pyobj(kwargs);
        match kwargs_handle {
            Some(bits) => bits,
            None => {
                crate::capi_trace::record_silent_failure(
                    "PyObject_Call",
                    Some("unresolved kwargs dict"),
                );
                if !exception_already_pending() {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_SystemError,
                            c"PyObject_Call: kwargs dict is not a bridge-managed object".as_ptr(),
                        );
                    }
                }
                return ptr::null_mut();
            }
        }
    };
    let h = hooks_or_stubs();
    let result_bits = unsafe { (h.object_call)(callable_bits, args_bits, kwargs_bits) };
    if let Some(bits) = owned_args_bits {
        unsafe { (h.dec_ref)(bits) };
    }
    if result_bits == 0 {
        // Runtime raised (pending exception) or hooks are unregistered.
        // Guarantee the NULL-return-carries-an-exception ABI contract.
        if !exception_already_pending() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"PyObject_Call failed: runtime call authority unavailable".as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result_bits) }
}

/// Marshal a C-layout tuple (`PyTupleObject` minted by the shim's
/// `PyTuple_New`/`PyTuple_Pack`) into a fresh, owned Molt tuple handle. Every
/// item must resolve through the bridge by identity — bridge proxies, static
/// singletons, or raw-registered opaque tokens (the established
/// container-crossing representation for extension-owned C objects). Returns
/// `None` (caller fails loudly) when `args` has no C tuple layout or an item
/// does not resolve.
unsafe fn molt_tuple_bits_from_c_tuple(args: *mut PyObject) -> Option<u64> {
    let tuple = unsafe { crate::api::sequences::tuple_layout_object(args) }?;
    let len = unsafe { (*tuple).ob_base.ob_size };
    if len < 0 {
        return None;
    }
    let n = len as usize;
    let items = unsafe { (*tuple).ob_item };
    if n > 0 && items.is_null() {
        return None;
    }
    let mut item_bits = Vec::with_capacity(n);
    {
        let mut bridge = GLOBAL_BRIDGE.lock();
        for i in 0..n {
            let item = unsafe { *items.add(i) };
            if item.is_null() {
                return None;
            }
            // Cross each argument INTO Molt as a first-class value: bridge
            // proxies / singletons resolve to their Molt handle, and a genuine
            // C-extension object gets a `TYPE_ID_FOREIGN` wrapper so the callee
            // can `getattr`/call it. Each is an owned reference the fresh Molt
            // tuple takes ownership of (released when the tuple is dropped).
            item_bits.push(unsafe { bridge.molt_value_for_pyobj(item) }?);
        }
    }
    let h = hooks_or_stubs();
    let tuple_bits = unsafe { (h.alloc_tuple)(n) };
    if tuple_bits == 0 {
        return None;
    }
    for (i, bits) in item_bits.into_iter().enumerate() {
        unsafe { (h.tuple_set)(tuple_bits, i, bits) };
    }
    Some(tuple_bits)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallObject(
    callable: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    // CPython contract (Objects/call.c::PyObject_CallObject): when `args` is
    // NULL the object is called with NO arguments via `_PyObject_CallNoArgs`,
    // which hands the callee's `tp_call` the empty-tuple singleton — a real
    // tuple, NEVER a NULL `args` pointer. numpy's `use_new_as_default`
    // (dtypemeta.c) depends on this: it does
    // `PyObject_CallObject((PyObject *)DTypeClass, NULL)` and the DType's
    // `tp_new` (e.g. numpy `stringdtype_new`) parses `args` as a tuple. Passing
    // the NULL straight through to `tp_new` violates the contract and strands
    // parametric DType constructors, so route NULL args through `CallNoArgs`.
    if args.is_null() {
        return unsafe { PyObject_CallNoArgs(callable) };
    }
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

/// Read a callable's `vectorcallfunc`, mirroring CPython 3.12
/// `_PyVectorcall_FunctionInline` (`Include/internal/pycore_call.h`) and the
/// public `PyVectorcall_Function` (`Objects/call.c`): return the pointer stored
/// at `Py_TYPE(callable)->tp_vectorcall_offset` inside the object, but ONLY when
/// the type advertises `Py_TPFLAGS_HAVE_VECTORCALL`. Returns `None` (CPython's
/// NULL — the "fall back to `tp_call`" signal) when the flag is absent, the
/// offset is non-positive, or the per-object slot itself is NULL.
///
/// The slot read is unaligned on purpose: a statically-declared C object on
/// wasm32 is only 4-byte aligned, so its pointer-sized `vectorcall` field can be
/// under-aligned relative to `align_of::<*const ()>()` (the `bridge.rs`
/// wasm32-alignment class). `read_unaligned` is correct on every target.
unsafe fn vectorcall_function(callable: *mut PyObject) -> Option<PyVectorcallFunc> {
    if callable.is_null() {
        return None;
    }
    let tp = unsafe { (*callable).ob_type };
    if tp.is_null() {
        return None;
    }
    if unsafe { (*tp).tp_flags } & Py_TPFLAGS_HAVE_VECTORCALL == 0 {
        return None;
    }
    let offset = unsafe { (*tp).tp_vectorcall_offset };
    if offset <= 0 {
        return None;
    }
    // memcpy(&ptr, (char *)callable + offset, sizeof(ptr)); a NULL slot decodes
    // to `None` via the fn-pointer niche, which the callers treat as "no slot".
    let slot = unsafe { (callable as *const u8).add(offset as usize) }
        .cast::<Option<PyVectorcallFunc>>();
    unsafe { ptr::read_unaligned(slot) }
}

/// CPython 3.12 `PyVectorcall_Function` (`Objects/call.c`): public accessor for a
/// callable's `vectorcallfunc` (or NULL). Declared in `include/Python.h` and
/// consumed by Cython's `__Pyx_PyVectorcall_Function` fast path (scipy/pandas);
/// previously absent from the runtime — an undefined-symbol build break for
/// Cython-generated modules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyVectorcall_Function(
    callable: *mut PyObject,
) -> Option<PyVectorcallFunc> {
    unsafe { vectorcall_function(callable) }
}

/// Mirror CPython `_Py_CheckFunctionResult` for the vectorcall paths: a NULL
/// return MUST carry a pending exception (the ABI contract). If a slot returned
/// NULL without setting one, raise `SystemError` so a C caller's canonical
/// `res == NULL && PyErr_Occurred()` check is honoured instead of stranding on a
/// bare NULL (a `SystemError: NULL result without error` / wrong-answer crash).
unsafe fn check_vectorcall_result(
    callable: *mut PyObject,
    result: *mut PyObject,
) -> *mut PyObject {
    if result.is_null() && !exception_already_pending() {
        let type_name = unsafe { type_name_lossy(callable) };
        crate::capi_trace::record_silent_failure("PyObject_Vectorcall", Some(&type_name));
        let message =
            format!("vectorcall of '{type_name}' object returned NULL without setting an exception");
        if let Ok(cmessage) = std::ffi::CString::new(message) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    cmessage.as_ptr(),
                );
            }
        }
    }
    result
}

/// Flatten a positional args array (`args[0..nargs]`, borrowed) plus a non-empty
/// `kwargs` dict into a single vectorcall stack + a `kwnames` tuple, then invoke
/// `func`. Mirrors CPython `_PyStack_UnpackDict` + the trailing `func(...)`:
/// the stack reserves one slot at the front so `PY_VECTORCALL_ARGUMENTS_OFFSET`
/// is set and the callee may borrow `args[-1]` as `self` scratch.
unsafe fn vectorcall_with_kwargs_dict(
    func: PyVectorcallFunc,
    callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargs: isize,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let nkw = unsafe { crate::api::mapping::PyDict_Size(kwargs) };
    if nkw < 0 {
        return ptr::null_mut();
    }
    let kwnames = unsafe { crate::api::sequences::PyTuple_New(nkw) };
    if kwnames.is_null() {
        return ptr::null_mut();
    }
    // Layout: [reserved][pos0..pos_{nargs-1}][kwval0..kwval_{nkw-1}].
    let mut stack: Vec<*mut PyObject> =
        Vec::with_capacity(1 + nargs.max(0) as usize + nkw as usize);
    stack.push(ptr::null_mut());
    for i in 0..nargs {
        stack.push(unsafe { *args.add(i as usize) });
    }
    let mut pos: Py_ssize_t = 0;
    let mut key: *mut PyObject = ptr::null_mut();
    let mut value: *mut PyObject = ptr::null_mut();
    let mut idx: isize = 0;
    while unsafe {
        crate::api::mapping::PyDict_Next(kwargs, &raw mut pos, &raw mut key, &raw mut value)
    } != 0
    {
        // `PyTuple_SetItem` steals a ref; `PyDict_Next` hands back a borrowed key.
        unsafe { crate::api::refcount::Py_INCREF(key) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(kwnames, idx, key) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(kwnames) };
            return ptr::null_mut();
        }
        stack.push(value);
        idx += 1;
    }
    // Args pointer starts AFTER the reserved slot; ARGUMENTS_OFFSET signals it.
    let args_ptr = unsafe { stack.as_ptr().add(1) } as *mut *mut PyObject;
    let result = unsafe {
        func(
            callable,
            args_ptr,
            (nargs as usize) | PY_VECTORCALL_ARGUMENTS_OFFSET,
            kwnames,
        )
    };
    unsafe { crate::api::refcount::Py_DECREF(kwnames) };
    // `stack` (and thus the reserved slot the callee may have written) stays live
    // until here — do not drop it before `func` returns.
    unsafe { check_vectorcall_result(callable, result) }
}

/// CPython 3.12 `_PyVectorcall_Call` (`Objects/call.c`): invoke `func` from a
/// positional-args tuple + optional `kwargs` dict. The no-keyword fast path
/// passes the tuple's items with a plain `nargs` (NO `ARGUMENTS_OFFSET`); the
/// keyword path flattens the dict via `vectorcall_with_kwargs_dict`. Never
/// re-enters `PyObject_Call`, so it cannot recurse through `tp_call`.
unsafe fn vectorcall_call_with_tuple(
    func: PyVectorcallFunc,
    callable: *mut PyObject,
    args_tuple: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let nargs = if args_tuple.is_null() {
        0
    } else {
        unsafe { crate::api::sequences::PyTuple_Size(args_tuple) }.max(0)
    };
    // Collect the tuple's items (borrowed) into a flat stack — CPython uses the
    // zero-copy `_PyTuple_ITEMS`; a `Vec` is correct for molt's dual-path
    // (ABI-layout and bridge-managed) tuples and is bounded by this frame.
    let mut pos_args: Vec<*mut PyObject> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs {
        pos_args.push(unsafe { crate::api::sequences::PyTuple_GetItem(args_tuple, i) });
    }
    let has_kwargs =
        !kwargs.is_null() && unsafe { crate::api::mapping::PyDict_Size(kwargs) } > 0;
    if !has_kwargs {
        let result = unsafe { func(callable, pos_args.as_mut_ptr(), nargs as usize, ptr::null_mut()) };
        return unsafe { check_vectorcall_result(callable, result) };
    }
    unsafe { vectorcall_with_kwargs_dict(func, callable, pos_args.as_mut_ptr(), nargs, kwargs) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Vectorcall(
    callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    if callable.is_null() {
        return ptr::null_mut();
    }
    // PEP-590 fast path (CPython `_PyObject_VectorcallTstate`): if the callable
    // carries a `vectorcallfunc`, call it DIRECTLY with the caller's argument
    // array + `kwnames` tuple. `nargsf` is forwarded UNMASKED — the callee
    // applies `PyVectorcall_NARGS` itself and may read
    // `PY_VECTORCALL_ARGUMENTS_OFFSET`.
    if let Some(func) = unsafe { vectorcall_function(callable) } {
        let result = unsafe { func(callable, args, nargsf, kwnames) };
        return unsafe { check_vectorcall_result(callable, result) };
    }
    // No vectorcall slot → `_PyObject_MakeTpCall`: build a positional tuple and
    // (when `kwnames` is non-empty) a keyword dict, then route through
    // `PyObject_Call`. We are here only because `callable` has NO slot, so
    // `PyObject_Call`'s own vectorcall-first probe also misses — no re-entry.
    unsafe { vectorcall_tpcall_fallback(callable, args, nargsf, kwnames) }
}

/// CPython `_PyObject_MakeTpCall` shape (the vectorcall→`tp_call` slow path):
/// materialise a positional args tuple from the vectorcall stack, a keyword dict
/// from the `kwnames` tuple (`kwnames[i] -> args[nargs + i]`), then dispatch via
/// `PyObject_Call` (which itself handles a real `tp_call` and molt's
/// bridge-managed callables). Refcount-clean: temporaries are released here.
unsafe fn vectorcall_tpcall_fallback(
    callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    let nargs = vectorcall_nargs(nargsf);
    let argstuple = unsafe { tuple_from_vectorcall_args(args, nargs) };
    if argstuple.is_null() {
        return ptr::null_mut();
    }
    let mut kwdict: *mut PyObject = ptr::null_mut();
    if !kwnames.is_null() {
        let nkw = unsafe { crate::api::sequences::PyTuple_Size(kwnames) };
        if nkw > 0 {
            kwdict = unsafe { crate::api::mapping::PyDict_New() };
            if kwdict.is_null() {
                unsafe { crate::api::refcount::Py_DECREF(argstuple) };
                return ptr::null_mut();
            }
            for i in 0..nkw {
                let name = unsafe { crate::api::sequences::PyTuple_GetItem(kwnames, i) };
                let value = unsafe { *args.add((nargs + i) as usize) };
                if name.is_null()
                    || value.is_null()
                    || unsafe { crate::api::mapping::PyDict_SetItem(kwdict, name, value) } != 0
                {
                    unsafe {
                        crate::api::refcount::Py_DECREF(argstuple);
                        crate::api::refcount::Py_DECREF(kwdict);
                    }
                    return ptr::null_mut();
                }
            }
        }
    }
    let result = unsafe { PyObject_Call(callable, argstuple, kwdict) };
    unsafe { crate::api::refcount::Py_DECREF(argstuple) };
    if !kwdict.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(kwdict) };
    }
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
    nargsf: usize,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    if callable.is_null() {
        return ptr::null_mut();
    }
    // CPython `_PyObject_FastCallDictTstate`: MASK `PY_VECTORCALL_ARGUMENTS_OFFSET`
    // out of `nargsf` — the previous `nargs as isize` leaked the high offset bit
    // into a negative / bogus count and dropped the call. Vectorcall-first, else
    // the `_PyObject_MakeTpCall` tuple/dict fallback.
    let nargs = vectorcall_nargs(nargsf);
    if let Some(func) = unsafe { vectorcall_function(callable) } {
        let has_kwargs =
            !kwargs.is_null() && unsafe { crate::api::mapping::PyDict_Size(kwargs) } > 0;
        if !has_kwargs {
            let result = unsafe { func(callable, args, nargsf, ptr::null_mut()) };
            return unsafe { check_vectorcall_result(callable, result) };
        }
        return unsafe { vectorcall_with_kwargs_dict(func, callable, args, nargs, kwargs) };
    }
    let argstuple = unsafe { tuple_from_vectorcall_args(args, nargs) };
    if argstuple.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyObject_Call(callable, argstuple, kwargs) };
    unsafe { crate::api::refcount::Py_DECREF(argstuple) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyVectorcall_Call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    if callable.is_null() {
        return ptr::null_mut();
    }
    // CPython 3.12 `PyVectorcall_Call` (`Objects/call.c`): read
    // `tp_vectorcall_offset` DIRECTLY — deliberately WITHOUT the
    // `Py_TPFLAGS_HAVE_VECTORCALL` gate, because this function is the documented
    // value for a vectorcall type's `tp_call`, so the object already claims
    // support. A missing offset or NULL slot raises `TypeError`; it NEVER
    // re-enters `PyObject_Call`, so `tp_call = PyVectorcall_Call` TERMINATES
    // instead of `tp_call → PyVectorcall_Call → PyObject_Call → tp_call → …`.
    let tp = unsafe { (*callable).ob_type };
    let offset = if tp.is_null() {
        0
    } else {
        unsafe { (*tp).tp_vectorcall_offset }
    };
    let func = if offset > 0 {
        let slot = unsafe { (callable as *const u8).add(offset as usize) }
            .cast::<Option<PyVectorcallFunc>>();
        unsafe { ptr::read_unaligned(slot) }
    } else {
        None
    };
    let Some(func) = func else {
        if !exception_already_pending() {
            let type_name = unsafe { type_name_lossy(callable) };
            crate::capi_trace::record_silent_failure("PyVectorcall_Call", Some(&type_name));
            let message = format!("'{type_name}' object does not support vectorcall");
            if let Ok(cmessage) = std::ffi::CString::new(message) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_TypeError,
                        cmessage.as_ptr(),
                    );
                }
            }
        }
        return ptr::null_mut();
    };
    unsafe { vectorcall_call_with_tuple(func, callable, args, kwargs) }
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
    // When `derived` and `cls` are type objects, CPython's `PyObject_IsSubclass`
    // reduces to `PyType_IsSubtype((PyTypeObject *)derived, (PyTypeObject *)cls)`
    // (Objects/abstract.c -> `recursive_issubclass`) — the C-extension case. A
    // bare pointer-identity check dropped every genuine base/derived
    // relationship (the same class of bug that stranded numpy's
    // `PyObject_TypeCheck`-based `PyArray_DescrCheck`). `PyType_IsSubtype`
    // already answers the exact-match case (`a == b`) and then walks
    // `derived`'s `tp_base` chain; it only pointer-compares `cls`, so a
    // non-type `cls` (the `__subclasscheck__` case Molt cannot resolve here)
    // yields the same conservative `0` rather than a false positive.
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(
            derived.cast::<PyTypeObject>(),
            cls.cast::<PyTypeObject>(),
        )
    }
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
    let self_ = unsafe { (*cfunc).m_self };

    if flags & METH_METHOD != 0 {
        // METH_METHOD (defining-class convention) is served by the vectorcall
        // path, not this tp_call bridge — fail loud rather than a bare NULL.
        return unsafe {
            cfunction_error(
                &raw mut crate::abi_types::PyExc_SystemError,
                format!(
                    "{}() uses the METH_METHOD calling convention, unsupported here",
                    cfunction_name(cfunc)
                ),
            )
        };
    }

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
            return unsafe { cfunction_no_kwargs_error(cfunc) };
        }
        let func: PyCFunctionFast = unsafe { std::mem::transmute(raw_func) };
        return unsafe { func(self_, ptr, items.len() as Py_ssize_t) };
    }

    if flags & METH_KEYWORDS != 0 {
        let func: PyCFunctionWithKeywords = unsafe { std::mem::transmute(raw_func) };
        return unsafe { func(self_, args, kwargs) };
    }
    if !kwargs.is_null() {
        return unsafe { cfunction_no_kwargs_error(cfunc) };
    }
    if flags & METH_NOARGS != 0 {
        let given = unsafe { tuple_arg_len(args) };
        if given != Some(0) {
            let n = given.unwrap_or(0);
            return unsafe {
                cfunction_error(
                    &raw mut crate::abi_types::PyExc_TypeError,
                    format!("{}() takes no arguments ({n} given)", cfunction_name(cfunc)),
                )
            };
        }
        return unsafe { raw_func(self_, ptr::null_mut()) };
    }
    if flags & METH_O != 0 {
        let given = unsafe { tuple_arg_len(args) };
        if given != Some(1) {
            let n = given.unwrap_or(0);
            return unsafe {
                cfunction_error(
                    &raw mut crate::abi_types::PyExc_TypeError,
                    format!(
                        "{}() takes exactly one argument ({n} given)",
                        cfunction_name(cfunc)
                    ),
                )
            };
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
    // Unknown/unsupported flag combination: fail loud, never a silent NULL.
    unsafe {
        cfunction_error(
            &raw mut crate::abi_types::PyExc_SystemError,
            format!("{}() has unsupported METH flags {flags:#x}", cfunction_name(cfunc)),
        )
    }
}

/// Best-effort `__name__` of a `PyCFunctionObject` for error messages.
unsafe fn cfunction_name(cfunc: *mut PyCFunctionObject) -> String {
    let ml = unsafe { (*cfunc).m_ml };
    if ml.is_null() {
        return "function".to_string();
    }
    let name = unsafe { (*ml).ml_name };
    if name.is_null() {
        return "function".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// Set `exc(message)` for a `cfunction_call` arg/flag mismatch and return NULL,
/// recording the site. No-op on the exception when one is already pending, so a
/// callee-set error is never clobbered. (CPython `Objects/methodobject.c`.)
unsafe fn cfunction_error(exc: *mut PyObject, message: String) -> *mut PyObject {
    crate::capi_trace::record_silent_failure("molt_cfunction_call", None);
    if !exception_already_pending()
        && let Ok(cmsg) = std::ffi::CString::new(message)
    {
        unsafe { crate::api::errors::PyErr_SetString(exc, cmsg.as_ptr()) };
    }
    ptr::null_mut()
}

/// CPython's `%U takes no keyword arguments` TypeError for a non-KEYWORDS method.
unsafe fn cfunction_no_kwargs_error(cfunc: *mut PyCFunctionObject) -> *mut PyObject {
    unsafe {
        cfunction_error(
            &raw mut crate::abi_types::PyExc_TypeError,
            format!("{}() takes no keyword arguments", cfunction_name(cfunc)),
        )
    }
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

    // Preferred path: register the C method as a real Molt callable through the
    // runtime, then return the *bridge-registered* PyObject view of it. This is
    // what lets the returned object resolve back to a Molt handle via
    // `pyobj_to_handle` — without it, `PyDict_SetItem(dict, name, func)` (the
    // tp_dict method-population step of PyType_Ready) cannot resolve `func` and
    // the descriptor is silently dropped. Falls back to a raw ABI-owned
    // `PyCFunctionObject` only when no runtime is wired (pure-ABI unit tests) or
    // the runtime rejects the method's flags.
    let ml_ref = unsafe { &*ml };
    if let Some(fn_ptr) = ml_ref.ml_meth {
        let name_bytes: &[u8] = if ml_ref.ml_name.is_null() {
            b""
        } else {
            unsafe { std::ffi::CStr::from_ptr(ml_ref.ml_name) }.to_bytes()
        };
        let self_bits = if self_.is_null() {
            none_bits()
        } else {
            let self_handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(self_);
            match self_handle {
                Some(bits) => bits,
                None => unsafe { crate::bridge::read_bridge_header_bits(self_) },
            }
        };
        let meth_addr = fn_ptr as *const () as usize as u64;
        let h = hooks_or_stubs();
        let func_bits = unsafe {
            (h.register_c_function)(
                meth_addr,
                ml_ref.ml_flags,
                self_bits,
                name_bytes.as_ptr(),
                name_bytes.len(),
            )
        };
        if func_bits != 0 {
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(func_bits) };
        }
    }

    // Fallback: no runtime-backed callable available. Return a raw ABI-owned
    // PyCFunctionObject so callers still get a non-null, callable-shaped object.
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

/// `_Py_NoneStruct` — the canonical CPython data symbol for `None` (genuine
/// CPython headers define `#define Py_None (&_Py_NoneStruct)`). Immortal via the
/// ONE authority and typed `PyNone_Type` so `Py_TYPE(Py_None)` is non-null (was
/// `ob_type = NULL` → null-deref); `bridge::pyobj_to_handle_static` resolves it
/// to the SAME canonical `None` handle as the molt-header `Py_None`, so
/// `_Py_NoneStruct is None` holds across the header boundary (matrix L1 #3/#4).
#[unsafe(no_mangle)]
pub static mut _Py_NoneStruct: PyObject = PyObject {
    ob_refcnt: crate::abi_types::IMMORTAL_REFCNT,
    ob_type: &raw mut crate::abi_types::PyNone_Type,
};

/// `_Py_TrueStruct` — canonical CPython `True`, a real value-carrying
/// `PyLongObject` (CPython v3.12.0 Objects/boolobject.c:
/// `PyObject_HEAD_INIT(&PyBool_Type) { .lv_tag = _PyLong_TRUE_TAG, { 1 } }`), so
/// an extension's inlined `((PyLongObject*)Py_True)->long_value.ob_digit[0]`
/// reads `1` IN BOUNDS instead of OOB past a bare `PyObject` (matrix L1 #5).
/// `_PyLong_TRUE_TAG = TAG_FROM_SIGN_AND_SIZE(1,1) = (1-1)|(1<<3) = 8`
/// (Include/internal/pycore_long.h). Resolves to the canonical `True` handle.
#[unsafe(no_mangle)]
pub static mut _Py_TrueStruct: crate::abi_types::PyLongObject = crate::abi_types::PyLongObject {
    ob_base: PyObject {
        ob_refcnt: crate::abi_types::IMMORTAL_REFCNT,
        ob_type: &raw mut crate::abi_types::PyBool_Type,
    },
    long_value: crate::abi_types::PyLongValue {
        lv_tag: 8,
        ob_digit: [1],
    },
};

/// `_Py_FalseStruct` — canonical CPython `False`, a `PyLongObject` with
/// `_PyLong_FALSE_TAG = TAG_FROM_SIGN_AND_SIZE(0,0) = (1-0)|(0<<3) = 1` and
/// `ob_digit[0] = 0` (CPython v3.12.0 boolobject.c / pycore_long.h).
#[unsafe(no_mangle)]
pub static mut _Py_FalseStruct: crate::abi_types::PyLongObject = crate::abi_types::PyLongObject {
    ob_base: PyObject {
        ob_refcnt: crate::abi_types::IMMORTAL_REFCNT,
        ob_type: &raw mut crate::abi_types::PyBool_Type,
    },
    long_value: crate::abi_types::PyLongValue {
        lv_tag: 1,
        ob_digit: [0],
    },
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
        // CPython returns b"<NULL>" here; a NULL argument is a caller bug, so we
        // keep the defensive NULL return (a valid failure sentinel), not a
        // fabricated value.
        return ptr::null_mut();
    }
    // Already bytes: return a new reference (CPython PyBytes_CheckExact fast path;
    // PyBytes_Check is the closest available predicate).
    if unsafe { crate::api::strings::PyBytes_Check(o) } != 0 {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    // Dispatch the object's `__bytes__`, as CPython's `PyObject_Bytes`
    // (Objects/object.c) does via `_PyObject_LookupSpecial`. The prior code
    // fabricated an empty `b''` for every non-bytes object — a silently-wrong
    // result (M05 poison) that masks the real conversion. `GetOptionalAttrString`
    // is the quiet lookup: >0 found (owned ref), 0 absent (no exception set), -1
    // error.
    let mut func: *mut PyObject = ptr::null_mut();
    let rc = unsafe {
        PyObject_GetOptionalAttrString(o, c"__bytes__".as_ptr(), &raw mut func)
    };
    if rc < 0 {
        return ptr::null_mut();
    }
    if rc > 0 && !func.is_null() {
        let result = unsafe { PyObject_CallNoArgs(func) };
        unsafe { crate::api::refcount::Py_DECREF(func) };
        if result.is_null() {
            return ptr::null_mut();
        }
        if unsafe { crate::api::strings::PyBytes_Check(result) } == 0 {
            let message = format!(
                "__bytes__ returned non-bytes (type {})",
                unsafe { type_name_lossy(result) }
            );
            unsafe { crate::api::refcount::Py_DECREF(result) };
            return unsafe { item_type_error_obj(o, "PyObject_Bytes", message) };
        }
        return result;
    }
    // No `__bytes__`: CPython falls back to `PyBytes_FromObject` (buffer protocol
    // / iterable-of-ints). Molt does not yet implement that fallback here; raise
    // the honest CPython-shaped TypeError instead of a fabricated value, and
    // record the site so the buffer/iterable path is a tracked gap, never silent.
    let message = format!(
        "cannot convert '{}' object to bytes",
        unsafe { type_name_lossy(o) }
    );
    unsafe { item_type_error_obj(o, "PyObject_Bytes", message) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_ASCII(o: *mut PyObject) -> *mut PyObject {
    // CPython `Objects/object.c PyObject_ASCII`: take `repr(o)` and backslash-
    // escape every non-ASCII code point, yielding a pure-ASCII str. The prior
    // alias returned non-ASCII code points literally.
    let repr = unsafe { crate::api::typeobj::PyObject_Repr(o) };
    if repr.is_null() {
        return ptr::null_mut();
    }
    let mut len: Py_ssize_t = 0;
    let text_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(repr, &raw mut len) };
    if text_ptr.is_null() || len < 0 {
        unsafe { crate::api::refcount::Py_DECREF(repr) };
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(text_ptr as *const u8, len as usize) };
    let text = String::from_utf8_lossy(bytes);
    // Fast path: an already-ASCII repr is returned unchanged (new reference).
    if text.is_ascii() {
        return repr;
    }
    let escaped = ascii_escape(&text);
    unsafe { crate::api::refcount::Py_DECREF(repr) };
    match std::ffi::CString::new(escaped) {
        Ok(c) => unsafe { crate::api::strings::PyUnicode_FromString(c.as_ptr()) },
        Err(_) => ptr::null_mut(),
    }
}

/// Backslash-escape every non-ASCII code point of `s` as CPython's `ascii()`:
/// `\xNN` (<= 0xFF), `\uNNNN` (<= 0xFFFF), else `\UNNNNNNNN`. ASCII passes through.
fn ascii_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp < 0x80 {
            out.push(ch);
        } else if cp <= 0xFF {
            out.push_str(&format!("\\x{cp:02x}"));
        } else if cp <= 0xFFFF {
            out.push_str(&format!("\\u{cp:04x}"));
        } else {
            out.push_str(&format!("\\U{cp:08x}"));
        }
    }
    out
}

#[cfg(test)]
mod item_access_slot_tests {
    //! Gate tests for the item-access protocol (`PyObject_GetItem` /
    //! `PyObject_SetItem`) foreign-slot dispatch. These mirror
    //! `abstract_number::conversion_slot_tests`: a foreign object's own type
    //! slots must be dispatched, and an unresolvable path must record the site
    //! on the silent-failure surface and set an honest exception — never a bare
    //! NULL / -1. They run on `STUB_HOOKS` (no runtime), reading the fake type's
    //! slots directly exactly as production reads a numpy/foreign object's.
    use super::*;
    use crate::abi_types::{PyMappingMethods, PyObject, PySequenceMethods, PyTypeObject};
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    // Stand-in "looked-up value" the fake slots hand back. Contents irrelevant;
    // tests only check pointer identity.
    static mut FAKE_ITEM_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };

    unsafe extern "C" fn fake_mp_subscript(
        _o: *mut PyObject,
        _key: *mut PyObject,
    ) -> *mut PyObject {
        &raw mut FAKE_ITEM_RESULT
    }

    /// (a) A foreign object whose type exposes `mp_subscript` must have
    /// `PyObject_GetItem` dispatch to that slot (the numpy-mapping path the
    /// prior bare-NULL return skipped entirely).
    #[test]
    fn get_item_dispatches_to_foreign_mp_subscript() {
        let _ = crate::capi_trace::take_last_silent_failure();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut mapping: PyMappingMethods = unsafe { std::mem::zeroed() };
        mapping.mp_subscript = fake_mp_subscript as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_mapping = (&raw mut mapping).cast::<c_void>();
        ty.tp_name = c"fake_mapping".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut key = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let result = unsafe { PyObject_GetItem(&raw mut obj, &raw mut key) };
        assert_eq!(result, &raw mut FAKE_ITEM_RESULT);
    }

    static SQ_ITEM_INDEX: AtomicI64 = AtomicI64::new(i64::MIN);

    unsafe extern "C" fn fake_sq_item(_o: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
        SQ_ITEM_INDEX.store(i as i64, Ordering::SeqCst);
        &raw mut FAKE_ITEM_RESULT
    }

    unsafe extern "C" fn fake_sq_length(_o: *mut PyObject) -> Py_ssize_t {
        5
    }

    /// The sequence lane of `PyObject_GetItem`: an index-like key is converted
    /// via `PyNumber_AsSsize_t` and a NEGATIVE index is adjusted by the object's
    /// own `sq_length` before `sq_item` is called (CPython `PySequence_GetItem`).
    /// This proves the disambiguation that a legitimate `-1` index is NOT the
    /// `PyNumber_AsSsize_t` error sentinel.
    #[test]
    fn get_item_sequence_path_adjusts_negative_index_via_sq_length() {
        unsafe { crate::api::errors::PyErr_Clear() };
        SQ_ITEM_INDEX.store(i64::MIN, Ordering::SeqCst);
        // A native int `-1` key so `PyIndex_Check`/`PyNumber_AsSsize_t` resolve it.
        let key = unsafe {
            GLOBAL_BRIDGE
                .lock()
                .handle_to_pyobj(MoltObject::from_int(-1).bits())
        };
        let mut seq: PySequenceMethods = unsafe { std::mem::zeroed() };
        seq.sq_item = fake_sq_item as *mut c_void;
        seq.sq_length = fake_sq_length as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_sequence = (&raw mut seq).cast::<c_void>();
        ty.tp_name = c"fake_sequence".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let result = unsafe { PyObject_GetItem(&raw mut obj, key) };
        assert_eq!(result, &raw mut FAKE_ITEM_RESULT);
        assert_eq!(
            SQ_ITEM_INDEX.load(Ordering::SeqCst),
            4,
            "negative index -1 must be adjusted to 4 via sq_length()==5"
        );
    }

    /// (b) A foreign object with NO mapping/sequence item slot must never return
    /// a bare NULL: `PyObject_GetItem` records the site on the permanent
    /// silent-failure surface and raises an honest `TypeError`.
    #[test]
    fn get_item_without_slot_is_never_a_silent_null() {
        let _ = crate::capi_trace::take_last_silent_failure();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"opaque".as_ptr();
        // tp_as_mapping and tp_as_sequence left NULL.
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut key = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let result = unsafe { PyObject_GetItem(&raw mut obj, &raw mut key) };
        assert!(result.is_null());
        let recorded = crate::capi_trace::take_last_silent_failure();
        assert!(
            recorded.as_deref().unwrap_or("").contains("PyObject_GetItem"),
            "expected PyObject_GetItem on the silent-failure surface, got {recorded:?}"
        );
        assert!(
            !unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "PyObject_GetItem must leave a pending exception, never a bare NULL"
        );
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    static SET_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn fake_mp_ass_subscript(
        _o: *mut PyObject,
        _key: *mut PyObject,
        _v: *mut PyObject,
    ) -> c_int {
        SET_CALLS.fetch_add(1, Ordering::SeqCst);
        0
    }

    /// (d) `PyObject_SetItem` foreign dispatch analog: a foreign object whose
    /// type exposes `mp_ass_subscript` must dispatch to it (return 0), not the
    /// prior bare -1.
    #[test]
    fn set_item_dispatches_to_foreign_mp_ass_subscript() {
        unsafe { crate::api::errors::PyErr_Clear() };
        SET_CALLS.store(0, Ordering::SeqCst);
        let mut mapping: PyMappingMethods = unsafe { std::mem::zeroed() };
        mapping.mp_ass_subscript = fake_mp_ass_subscript as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_mapping = (&raw mut mapping).cast::<c_void>();
        ty.tp_name = c"fake_mapping".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut key = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let mut val = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let rc = unsafe { PyObject_SetItem(&raw mut obj, &raw mut key, &raw mut val) };
        assert_eq!(rc, 0);
        assert_eq!(
            SET_CALLS.load(Ordering::SeqCst),
            1,
            "mp_ass_subscript must be dispatched exactly once"
        );
    }

    /// A foreign object with no item-assignment slot must never return a bare
    /// -1: record + honest `TypeError` (the SetItem analog of (b)).
    #[test]
    fn set_item_without_slot_is_never_a_silent_minus_one() {
        let _ = crate::capi_trace::take_last_silent_failure();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"opaque".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut key = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let mut val = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let rc = unsafe { PyObject_SetItem(&raw mut obj, &raw mut key, &raw mut val) };
        assert_eq!(rc, -1);
        let recorded = crate::capi_trace::take_last_silent_failure();
        assert!(
            recorded.as_deref().unwrap_or("").contains("PyObject_SetItem"),
            "expected PyObject_SetItem on the silent-failure surface, got {recorded:?}"
        );
        assert!(
            !unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "PyObject_SetItem must leave a pending exception, never a bare -1"
        );
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    /// The bytes fast path is preserved: `PyObject_Bytes` on a genuine bytes
    /// object returns a NEW reference to it (never a fabricated value). The
    /// non-bytes fabrication-removal is covered in the `test_item_access`
    /// integration binary, where `alloc_str` is wired so the `__bytes__` lookup
    /// reaches the honest TypeError path.
    #[test]
    fn object_bytes_passthrough_preserves_bytes_identity() {
        // A stand-in bytes object: its type IS `PyBytes_Type`, so `PyBytes_Check`
        // succeeds and the fast path returns it with an incremented refcount.
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyBytes_Type,
        };
        let result = unsafe { PyObject_Bytes(&raw mut obj) };
        assert_eq!(
            result,
            &raw mut obj,
            "PyObject_Bytes must return the same bytes object (passthrough)"
        );
        assert_eq!(obj.ob_refcnt, 2, "passthrough must take a new reference");
    }
}

/// Mask-proof gate for the CPYTHON-ABI-AUDIT lane F3 fixes in this file. Each
/// test asserts a CPython-3.12 semantic that a fix restored; a revert of that fix
/// flips the test red. Foreign-slot tests run on STUB hooks with fake types
/// (bridge miss → the foreign dispatch tier, exactly as a numpy object).
#[cfg(test)]
mod f3_divergence_tests {
    use super::*;
    use crate::abi_types::{
        PyMappingMethods, PyNumberMethods, PyObject, PySequenceMethods, PyTypeObject,
    };
    use std::sync::atomic::{AtomicI64, Ordering};

    static NB_BOOL_RET: AtomicI64 = AtomicI64::new(0);
    unsafe extern "C" fn fake_nb_bool(_o: *mut PyObject) -> c_int {
        NB_BOOL_RET.load(Ordering::SeqCst) as c_int
    }
    unsafe extern "C" fn fake_len_zero(_o: *mut PyObject) -> Py_ssize_t {
        0
    }
    unsafe extern "C" fn fake_len_three(_o: *mut PyObject) -> Py_ssize_t {
        3
    }

    /// `PyObject_IsTrue` must dispatch a foreign object's `nb_bool` (the numpy
    /// `bool(np.array(...))` path) — the prior `None => 1` reported every
    /// bridge-miss object as truthy without ever consulting a slot.
    #[test]
    fn is_true_dispatches_foreign_nb_bool() {
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut num: PyNumberMethods = unsafe { std::mem::zeroed() };
        num.nb_bool = fake_nb_bool as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_number = (&raw mut num).cast::<c_void>();
        ty.tp_name = c"fake_bool".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        NB_BOOL_RET.store(0, Ordering::SeqCst);
        assert_eq!(
            unsafe { PyObject_IsTrue(&raw mut obj) },
            0,
            "nb_bool()==0 must make the object falsy"
        );
        NB_BOOL_RET.store(1, Ordering::SeqCst);
        assert_eq!(
            unsafe { PyObject_IsTrue(&raw mut obj) },
            1,
            "nb_bool()==1 must make the object truthy"
        );
    }

    /// An empty foreign container (`mp_length`/`sq_length` == 0) is FALSY, and a
    /// non-empty one is truthy — CPython's mp_length/sq_length truthiness tier.
    #[test]
    fn is_true_empty_container_is_falsy_via_length_slots() {
        unsafe { crate::api::errors::PyErr_Clear() };
        // mp_length == 0 -> falsy.
        let mut mapping: PyMappingMethods = unsafe { std::mem::zeroed() };
        mapping.mp_length = fake_len_zero as *mut c_void;
        let mut ty_m: PyTypeObject = unsafe { std::mem::zeroed() };
        ty_m.tp_as_mapping = (&raw mut mapping).cast::<c_void>();
        ty_m.tp_name = c"fake_map".as_ptr();
        let mut empty = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty_m,
        };
        assert_eq!(
            unsafe { PyObject_IsTrue(&raw mut empty) },
            0,
            "mp_length()==0 must be falsy (bool of an empty container)"
        );
        // sq_length == 3 -> truthy.
        let mut seq: PySequenceMethods = unsafe { std::mem::zeroed() };
        seq.sq_length = fake_len_three as *mut c_void;
        let mut ty_s: PyTypeObject = unsafe { std::mem::zeroed() };
        ty_s.tp_as_sequence = (&raw mut seq).cast::<c_void>();
        ty_s.tp_name = c"fake_seq".as_ptr();
        let mut full = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty_s,
        };
        assert_eq!(
            unsafe { PyObject_IsTrue(&raw mut full) },
            1,
            "sq_length()==3 must be truthy"
        );
    }

    /// An object with no `nb_bool`/`mp_length`/`sq_length` is truthy (CPython
    /// default), never -1.
    #[test]
    fn is_true_no_slots_defaults_truthy() {
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"opaque".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        assert_eq!(unsafe { PyObject_IsTrue(&raw mut obj) }, 1);
    }

    /// `PyObject_Size` foreign path dispatches `sq_length` (the numpy
    /// `len(ndarray)` path) instead of the prior silent -1.
    #[test]
    fn size_dispatches_foreign_sq_length() {
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut seq: PySequenceMethods = unsafe { std::mem::zeroed() };
        seq.sq_length = fake_len_three as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_sequence = (&raw mut seq).cast::<c_void>();
        ty.tp_name = c"fake_seq".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        assert_eq!(unsafe { PyObject_Size(&raw mut obj) }, 3);
    }

    /// A slot-less object's `PyObject_Size` must raise the len() TypeError, never
    /// a bare -1.
    #[test]
    fn size_without_len_slot_raises_typeerror() {
        let _ = crate::capi_trace::take_last_silent_failure();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"opaque".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        assert_eq!(unsafe { PyObject_Size(&raw mut obj) }, -1);
        assert!(
            !unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "PyObject_Size(-1) must leave a pending exception (has no len())"
        );
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    unsafe extern "C" fn getattro_raises_memory(
        _o: *mut PyObject,
        _name: *mut PyObject,
    ) -> *mut PyObject {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_MemoryError,
                c"out of memory in __array__".as_ptr(),
            );
        }
        ptr::null_mut()
    }
    unsafe extern "C" fn getattro_raises_attribute(
        _o: *mut PyObject,
        _name: *mut PyObject,
    ) -> *mut PyObject {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_AttributeError,
                c"no such attribute".as_ptr(),
            );
        }
        ptr::null_mut()
    }

    /// `PyObject_GetOptionalAttr` must PROPAGATE a non-AttributeError from an
    /// optional-dunder getter (MemoryError from numpy `__array__`) as -1 with the
    /// exception pending — the prior unconditional `PyErr_Clear` swallowed it as
    /// "absent" (silent-wrong-answer on coercion). Load-bearing.
    #[test]
    fn get_optional_attr_propagates_non_attribute_error() {
        crate::bridge::molt_cpython_abi_init();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_getattro = Some(getattro_raises_memory);
        ty.tp_name = c"raiser".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut name = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let mut result: *mut PyObject = ptr::null_mut();
        let rc = unsafe { PyObject_GetOptionalAttr(&raw mut obj, &raw mut name, &raw mut result) };
        assert_eq!(rc, -1, "a MemoryError getter must propagate as -1, not 0");
        assert!(result.is_null());
        assert!(
            !unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "the MemoryError must stay pending, never swallowed as 'absent'"
        );
        assert_ne!(
            unsafe {
                crate::api::errors::PyErr_ExceptionMatches(
                    &raw mut crate::abi_types::PyExc_MemoryError,
                )
            },
            0,
            "the pending exception must still be MemoryError"
        );
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    /// The complement: an `AttributeError` getter means "absent" — cleared, return
    /// 0 (so a genuinely-missing optional dunder is still detected).
    #[test]
    fn get_optional_attr_absent_on_attribute_error() {
        crate::bridge::molt_cpython_abi_init();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_getattro = Some(getattro_raises_attribute);
        ty.tp_name = c"raiser".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut name = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let mut result: *mut PyObject = ptr::null_mut();
        let rc = unsafe { PyObject_GetOptionalAttr(&raw mut obj, &raw mut name, &raw mut result) };
        assert_eq!(rc, 0, "an AttributeError getter means absent (0)");
        assert!(result.is_null());
        assert!(
            unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "the AttributeError must be cleared on the absent path"
        );
    }

    /// `PyObject_ASCII` backslash-escapes non-ASCII code points (the load-bearing
    /// escaping logic) as CPython's `ascii()` does.
    #[test]
    fn ascii_escape_matches_cpython() {
        assert_eq!(ascii_escape("ABC"), "ABC");
        assert_eq!(ascii_escape("café"), "caf\\xe9");
        assert_eq!(ascii_escape("λ"), "\\u03bb");
        assert_eq!(ascii_escape("\u{1F600}"), "\\U0001f600");
    }

    /// `PySeqIter_New(NULL)` raises (BadInternalCall) rather than returning a
    /// silent NULL, and a real iterator object is produced for a non-NULL seq.
    #[test]
    fn seqiter_new_null_raises_and_nonnull_builds_iterator() {
        crate::bridge::molt_cpython_abi_init();
        unsafe { crate::api::errors::PyErr_Clear() };
        assert!(unsafe { PySeqIter_New(ptr::null_mut()) }.is_null());
        assert!(
            !unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "PySeqIter_New(NULL) must set an exception"
        );
        unsafe { crate::api::errors::PyErr_Clear() };
        // A non-NULL seq yields a distinct iterator object whose tp_iternext is
        // installed (PyIter_Check true) — not the seq itself.
        let mut seq = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let it = unsafe { PySeqIter_New(&raw mut seq) };
        assert!(!it.is_null());
        assert!(!std::ptr::eq(it, &raw mut seq), "must be a NEW iterator object");
        assert_eq!(
            unsafe { PyIter_Check(it) },
            1,
            "the sequence iterator must itself be an iterator"
        );
        assert_eq!(seq.ob_refcnt, 2, "the iterator holds a reference to the seq");
        unsafe { crate::api::refcount::Py_DECREF(it) };
        assert_eq!(seq.ob_refcnt, 1, "dealloc must release the seq reference");
    }
}
