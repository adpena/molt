//! Sequence API — PyList_*, PyTuple_*.

use crate::abi_types::{Py_ssize_t, PyObject, PyTupleObject, PyVarObject};
#[allow(unused_imports)]
use crate::abi_types::{PyList_Type, PyTuple_Type};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

// ─── PyList ───────────────────────────────────────────────────────────────

/// Resolve `op` to its runtime handle bits iff it is a Molt-native list.
/// `None` → the caller sets the CPython-shaped exception (`BadInternalCall`).
fn resolve_native_list(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    let bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(op)?;
    if !MoltObject::from_bits(bits).is_ptr() {
        return None;
    }
    let h = hooks_or_stubs();
    if unsafe { (h.classify_heap)(bits) } == crate::abi_types::MoltTypeTag::List as u8 {
        Some(bits)
    } else {
        None
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_New(size: Py_ssize_t) -> *mut PyObject {
    // CPython: negative size → SystemError (PyErr_BadInternalCall).
    if size < 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_list)() };
    if bits == 0 {
        // Allocation failed. CPython's PyList_New returns NULL with MemoryError
        // set. Returning Py_None (non-NULL) here would defeat the extension's
        // `if (list == NULL)` guard and let it operate on None as if it were a
        // list — silent corruption. Fail closed with NULL + a set exception.
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_MemoryError,
                c"PyList_New: failed to allocate list".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    // CPython pre-sizes the list to `size` NULL slots: PyList_GET_SIZE reports
    // `size` immediately and PyList_SetItem/SET_ITEM stores at ANY index in
    // [0,size) — including out of order. The previous body ignored `size`
    // (empty list) and SET_ITEM appended, silently mis-placing out-of-order
    // fills. Molt slots are pre-filled with None (the runtime has no NULL
    // slot; extensions must fill before reading either way).
    let none_bits = MoltObject::none().bits();
    for _ in 0..size {
        unsafe { (h.list_append)(bits, none_bits) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Append(list: *mut PyObject, item: *mut PyObject) -> c_int {
    // CPython: non-list or NULL newitem → PyErr_BadInternalCall() + -1. Every
    // -1 return must carry a pending exception (sentinel sweep).
    if list.is_null() || item.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let mut bridge = GLOBAL_BRIDGE.lock();
    let list_bits = match bridge.pyobj_to_handle(list) {
        Some(b) => b,
        None => {
            drop(bridge);
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return -1;
        }
    };
    let mut item_is_foreign = false;
    let item_bits = match bridge.pyobj_to_handle(item) {
        Some(b) => b,
        None => match unsafe { bridge.molt_value_for_pyobj(item) } {
            // A genuine C-extension object item: give it a first-class
            // `TYPE_ID_FOREIGN` wrapper so it can be stored in the Molt list.
            Some(b) => {
                item_is_foreign = true;
                b
            }
            None => {
                drop(bridge);
                // No foreign wrapper could be minted (runtime hooks absent).
                // Fail loud instead of a contentless -1.
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_SystemError,
                            c"PyList_Append: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.list_append)(list_bits, item_bits) };
    // CPython contract: `PyList_Append` takes its own strong reference to the
    // item (it does not steal). Anchor the item proxy so the extension's
    // balancing `Py_DECREF` cannot sever the pointer↔handle mapping while the
    // item stays reachable from the runtime list (same class as the
    // `PyDict_SetItem` anchor — see api/mapping.rs). A foreign-wrapped item
    // already holds its own strong reference on the C object (minted at
    // refcount 1, ownership transferred to the list), so it is not INCREF'd.
    if !item_is_foreign {
        unsafe { crate::api::refcount::Py_INCREF(item) };
    }
    0
}

/// Raw (macro-semantics) item read: no type/bounds exception, NULL on any miss.
/// Borrowed reference, exactly like CPython's `PyList_GET_ITEM` macro. The
/// checked entry point is [`PyList_GetItem`] (which the ABI tier's
/// `PyList_GET_ITEM` header macro also routes to).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if op.is_null() || i < 0 {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let item_bits = unsafe { (h.list_item)(bits, i as usize) };
    if item_bits == 0 {
        return ptr::null_mut();
    }
    // CPython returns a BORROWED reference; the previous owning
    // `handle_to_pyobj` over-anchored one proxy ref per read.
    unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(item_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    // CPython: non-list → PyErr_BadInternalCall; OOB (incl. negative) →
    // IndexError "list index out of range"; success → borrowed ref. The prior
    // delegation to GET_ITEM returned bare NULLs with no exception on both
    // error classes (silent-sentinel row).
    let bits = match resolve_native_list(op) {
        Some(b) => b,
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return ptr::null_mut();
        }
    };
    let h = hooks_or_stubs();
    if i >= 0 {
        // Valid list item bits are never 0, so a 0 read == out of range;
        // single hook call on the hot path.
        let item_bits = unsafe { (h.list_item)(bits, i as usize) };
        if item_bits != 0 {
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(item_bits) };
        }
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_IndexError,
            c"list index out of range".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItemRef(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    let item = unsafe { PyList_GetItem(op, i) };
    if item.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(item) };
    item
}

/// Macro-semantics indexed store (steals the reference to `v`). Routes to the
/// same real indexed `PyList_SetItem` store — the ABI tier's header maps the
/// `PyList_SET_ITEM` macro to `PyList_SetItem` anyway. The previous body
/// APPENDED via `list_append` (mis-placing any out-of-order fill and
/// duplicating on replace) and silently DROPPED foreign items.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SET_ITEM(op: *mut PyObject, i: Py_ssize_t, v: *mut PyObject) {
    unsafe {
        let _ = PyList_SetItem(op, i, v);
    }
}

/// Faithful `Objects/listobject.c` `PyList_SetItem`:
/// * steals the reference to `v` — released on EVERY error path (`Py_XDECREF`);
/// * non-list receiver → `PyErr_BadInternalCall()` + `-1`;
/// * out-of-range `i` (incl. negative) → `IndexError` "list assignment index
///   out of range" + `-1`;
/// * success → indexed store via the `list_set` hook (`Py_SETREF` semantics).
///
/// A foreign (C-extension) `v` gets first-class `TYPE_ID_FOREIGN` custody via
/// `molt_value_for_pyobj` (pattern from 04599327e2) — the wrapper takes its own
/// strong reference, and the stolen caller reference is consumed here — instead
/// of the old silent drop-and-report-success.
///
/// Custody note: the *replaced* occupant's bridge anchor is intentionally NOT
/// released — same deliberate leak-not-corrupt trade `PyDict_SetItem` documents
/// for removed entries (a mismatched release on a foreign wrapper would
/// double-free when the wrapper later drops; the anchor is a small header).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SetItem(
    op: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    let list_bits = match resolve_native_list(op) {
        Some(b) => b,
        None => {
            unsafe {
                crate::api::refcount::Py_XDECREF(v);
                crate::api::errors::PyErr_BadInternalCall();
            }
            return -1;
        }
    };
    if v.is_null() {
        // CPython would store a NULL slot; Molt lists have no NULL slot. An
        // honest SystemError beats fabricating a stored value.
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let mut bridge = GLOBAL_BRIDGE.lock();
    let mut val_is_foreign = false;
    let val_bits = match bridge.pyobj_to_handle(v) {
        Some(b) => b,
        None => match unsafe { bridge.molt_value_for_pyobj(v) } {
            Some(b) => {
                val_is_foreign = true;
                b
            }
            None => {
                drop(bridge);
                unsafe {
                    crate::api::refcount::Py_XDECREF(v);
                    if crate::api::errors::PyErr_Occurred().is_null() {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_SystemError,
                            c"PyList_SetItem: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let stored = if i >= 0 {
        unsafe { (h.list_set)(list_bits, i as usize, val_bits, ptr::null_mut()) }
    } else {
        0
    };
    if stored != 1 {
        // OOB: CPython Py_XDECREFs the stolen reference, then IndexError.
        unsafe {
            crate::api::refcount::Py_XDECREF(v);
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_IndexError,
                c"list assignment index out of range".as_ptr(),
            );
        }
        return -1;
    }
    // Steal contract on success: the container takes over the caller's
    // reference — a bridge proxy is NOT INCREF'd (unlike the non-stealing
    // Append), and a foreign value's stolen C reference is consumed now (the
    // TYPE_ID_FOREIGN wrapper holds its own strong reference for custody).
    if val_is_foreign {
        unsafe { crate::api::refcount::Py_DECREF(v) };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return 0;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return 0,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.list_len)(bits) as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Size(op: *mut PyObject) -> Py_ssize_t {
    // CPython: `if (!PyList_Check(op)) { PyErr_BadInternalCall(); return -1; }`.
    match resolve_native_list(op) {
        Some(bits) => {
            let h = hooks_or_stubs();
            unsafe { (h.list_len)(bits) as Py_ssize_t }
        }
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return 0;
    }
    if std::ptr::eq(ob_type, &raw const crate::abi_types::PyList_Type) {
        return 1;
    }
    // CPython PyList_Check = Py_TPFLAGS_LIST_SUBCLASS — list AND subclasses.
    // Same subtype-walk class as fcb1d9596d; identity-only was the ledger row.
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyList_Type)
    }
}

// ─── PyTuple ──────────────────────────────────────────────────────────────

pub(crate) unsafe fn tuple_layout_object(op: *mut PyObject) -> Option<*mut PyTupleObject> {
    if op.is_null() {
        return None;
    }
    let ob_type = unsafe { (*op).ob_type };
    if std::ptr::eq(ob_type, &raw mut crate::abi_types::PyTuple_Type)
        && GLOBAL_BRIDGE.lock().pyobj_to_handle(op).is_none()
    {
        Some(op.cast::<PyTupleObject>())
    } else {
        None
    }
}

pub unsafe extern "C" fn molt_tuple_dealloc(op: *mut PyObject) {
    let Some(tuple) = (unsafe { tuple_layout_object(op) }) else {
        return;
    };
    let len = unsafe { (*tuple).ob_base.ob_size };
    let item_ptr = unsafe { (*tuple).ob_item };
    if !item_ptr.is_null() && len > 0 {
        let items = unsafe { std::slice::from_raw_parts_mut(item_ptr, len as usize) };
        for item in items.iter_mut() {
            unsafe { crate::api::refcount::Py_XDECREF(*item) };
        }
        let slice = std::ptr::slice_from_raw_parts_mut(item_ptr, len as usize);
        unsafe { drop(Box::from_raw(slice)) };
    }
    unsafe { drop(Box::from_raw(tuple)) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_New(size: Py_ssize_t) -> *mut PyObject {
    let n = if size < 0 { 0 } else { size as usize };
    let items = vec![ptr::null_mut(); n].into_boxed_slice();
    let item_ptr = Box::leak(items).as_mut_ptr();
    let tuple = Box::new(PyTupleObject {
        ob_base: PyVarObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut crate::abi_types::PyTuple_Type,
            },
            ob_size: n as Py_ssize_t,
        },
        ob_item: item_ptr,
    });
    Box::into_raw(tuple).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_FromArray(
    array: *const *mut PyObject,
    size: Py_ssize_t,
) -> *mut PyObject {
    if size < 0 || (size > 0 && array.is_null()) {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let tuple = unsafe { PyTuple_New(size) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    for index in 0..size {
        let item = unsafe { *array.add(index as usize) };
        unsafe { crate::api::refcount::Py_XINCREF(item) };
        if unsafe { PyTuple_SetItem(tuple, index, item) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(tuple) };
            return ptr::null_mut();
        }
    }
    tuple
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if op.is_null() || i < 0 {
        return ptr::null_mut();
    }
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        let len = unsafe { (*tuple).ob_base.ob_size };
        if i >= len || unsafe { (*tuple).ob_item.is_null() } {
            return ptr::null_mut();
        }
        return unsafe { *(*tuple).ob_item.add(i as usize) };
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let item_bits = unsafe { (h.tuple_item)(bits, i as usize) };
    if item_bits == 0 {
        return ptr::null_mut();
    }
    // Borrowed reference (CPython macro contract), matching PyList_GET_ITEM.
    unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(item_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    // CPython Objects/tupleobject.c: non-tuple → PyErr_BadInternalCall; OOB
    // (incl. negative) → IndexError "tuple index out of range"; in-bounds →
    // the raw borrowed slot (may be NULL on a partially-filled tuple, with no
    // exception). The prior GET_ITEM delegation returned bare NULLs with no
    // exception for both error classes (silent-sentinel row).
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    // ABI-layout tuple: the extension-built hot path, zero hook calls.
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        let len = unsafe { (*tuple).ob_base.ob_size };
        if i < 0 || i >= len || unsafe { (*tuple).ob_item.is_null() } {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_IndexError,
                    c"tuple index out of range".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        return unsafe { *(*tuple).ob_item.add(i as usize) };
    }
    // Bridge-managed Molt tuple.
    let op_handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(op);
    let bits = match op_handle {
        Some(b) if MoltObject::from_bits(b).is_ptr() => b,
        _ => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return ptr::null_mut();
        }
    };
    let h = hooks_or_stubs();
    if unsafe { (h.classify_heap)(bits) } != crate::abi_types::MoltTypeTag::Tuple as u8 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if i >= 0 {
        // Valid tuple item bits are never 0, so 0 == out of range.
        let item_bits = unsafe { (h.tuple_item)(bits, i as usize) };
        if item_bits != 0 {
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(item_bits) };
        }
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_IndexError,
            c"tuple index out of range".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return 0;
    }
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        return unsafe { (*tuple).ob_base.ob_size };
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return 0,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.tuple_len)(bits) as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Size(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyTuple_GET_SIZE(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GetSlice(
    op: *mut PyObject,
    start: Py_ssize_t,
    end: Py_ssize_t,
) -> *mut PyObject {
    if unsafe { PyTuple_Check(op) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let len = unsafe { PyTuple_GET_SIZE(op) };
    let lo = start.clamp(0, len);
    let hi = end.clamp(lo, len);
    let out = unsafe { PyTuple_New(hi - lo) };
    if out.is_null() {
        return ptr::null_mut();
    }
    for index in lo..hi {
        let item = unsafe { PyTuple_GET_ITEM(op, index) };
        if item.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(out) };
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_INCREF(item) };
        if unsafe { PyTuple_SetItem(out, index - lo, item) } != 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(item);
                crate::api::refcount::Py_DECREF(out);
            }
            return ptr::null_mut();
        }
    }
    out
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyTuple_Resize(pv: *mut *mut PyObject, newsize: Py_ssize_t) -> c_int {
    if pv.is_null() || newsize < 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let op = unsafe { *pv };
    let Some(tuple) = (unsafe { tuple_layout_object(op) }) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    };
    if unsafe { (*op).ob_refcnt } != 1 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let oldsize = unsafe { (*tuple).ob_base.ob_size };
    let oldptr = unsafe { (*tuple).ob_item };
    let mut items = vec![ptr::null_mut(); newsize as usize].into_boxed_slice();
    let copied = oldsize.min(newsize);
    for index in 0..copied {
        items[index as usize] = unsafe { *oldptr.add(index as usize) };
    }
    if newsize < oldsize {
        for index in newsize..oldsize {
            unsafe { crate::api::refcount::Py_XDECREF(*oldptr.add(index as usize)) };
        }
    }
    if !oldptr.is_null() && oldsize > 0 {
        let slice = std::ptr::slice_from_raw_parts_mut(oldptr, oldsize as usize);
        unsafe { drop(Box::from_raw(slice)) };
    }
    unsafe {
        (*tuple).ob_item = Box::leak(items).as_mut_ptr();
        (*tuple).ob_base.ob_size = newsize;
    }
    0
}

/// Faithful `Objects/tupleobject.c` `PyTuple_SetItem`: steals the reference to
/// `v` and `Py_XDECREF`s it on EVERY error path; non-tuple → BadInternalCall;
/// OOB → IndexError "tuple assignment index out of range". The bridge tier is
/// bounds-checked before the store (the raw `tuple_set` hook auto-grows, which
/// would silently accept an OOB index). Foreign `v` gets `TYPE_ID_FOREIGN`
/// custody (same contract as `PyList_SetItem`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_SetItem(
    op: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    if op.is_null() || v.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(v);
            crate::api::errors::PyErr_BadInternalCall();
        }
        return -1;
    }
    // ABI-layout tuple: direct slot store (steal), zero hook calls.
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        let len = unsafe { (*tuple).ob_base.ob_size };
        if i < 0 || i >= len || unsafe { (*tuple).ob_item.is_null() } {
            unsafe {
                crate::api::refcount::Py_XDECREF(v);
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_IndexError,
                    c"tuple assignment index out of range".as_ptr(),
                );
            }
            return -1;
        }
        let slot = unsafe { (*tuple).ob_item.add(i as usize) };
        unsafe {
            let old = *slot;
            *slot = v;
            crate::api::refcount::Py_XDECREF(old);
        }
        return 0;
    }
    // Bridge-managed Molt tuple.
    let mut bridge = GLOBAL_BRIDGE.lock();
    let tuple_bits = match bridge.pyobj_to_handle(op) {
        Some(b) if MoltObject::from_bits(b).is_ptr() => b,
        _ => {
            drop(bridge);
            unsafe {
                crate::api::refcount::Py_XDECREF(v);
                crate::api::errors::PyErr_BadInternalCall();
            }
            return -1;
        }
    };
    let mut val_is_foreign = false;
    let val_bits = match bridge.pyobj_to_handle(v) {
        Some(b) => b,
        None => match unsafe { bridge.molt_value_for_pyobj(v) } {
            Some(b) => {
                val_is_foreign = true;
                b
            }
            None => {
                drop(bridge);
                unsafe {
                    crate::api::refcount::Py_XDECREF(v);
                    if crate::api::errors::PyErr_Occurred().is_null() {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_SystemError,
                            c"PyTuple_SetItem: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    drop(bridge);
    let h = hooks_or_stubs();
    // Bounds/type check first: the raw tuple_set hook grows the backing vector
    // on an OOB index instead of failing, so gate it here.
    let is_tuple =
        unsafe { (h.classify_heap)(tuple_bits) } == crate::abi_types::MoltTypeTag::Tuple as u8;
    let in_bounds = i >= 0 && is_tuple && (i as usize) < unsafe { (h.tuple_len)(tuple_bits) };
    if !in_bounds {
        unsafe {
            crate::api::refcount::Py_XDECREF(v);
            if is_tuple {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_IndexError,
                    c"tuple assignment index out of range".as_ptr(),
                );
            } else {
                crate::api::errors::PyErr_BadInternalCall();
            }
        }
        return -1;
    }
    unsafe { (h.tuple_set)(tuple_bits, i as usize, val_bits) };
    // Steal contract: a foreign value's stolen C reference is consumed now (the
    // wrapper holds its own strong reference); a bridge proxy's reference
    // transfers to the container un-INCREF'd.
    if val_is_foreign {
        unsafe { crate::api::refcount::Py_DECREF(v) };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return 0;
    }
    if std::ptr::eq(ob_type, &raw const crate::abi_types::PyTuple_Type) {
        return 1;
    }
    // CPython PyTuple_Check = Py_TPFLAGS_TUPLE_SUBCLASS — tuple AND subclasses
    // (same subtype-walk class as fcb1d9596d).
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyTuple_Type)
    }
}

/// Return `Py_True`/`Py_False` as a new reference (CPython richcompare contract).
#[inline]
fn seq_richcmp_bool(b: bool) -> *mut PyObject {
    let res = if b {
        (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
    } else {
        (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
    };
    unsafe { crate::api::refcount::Py_INCREF(res) };
    res
}

/// Element-wise (lexicographic) structural richcompare shared by the built-in
/// homogeneous sequence types (`tuple`, `list`). Both operands are already known
/// to be the concrete type; `len`/`get` are that type's dual-path size/item
/// accessors, so it is correct for both ABI-layout (`PyTuple_New`/`PyList_New`)
/// and bridge-managed runtime objects. Faithful port of CPython
/// `Objects/tupleobject.c tuplerichcompare` / `Objects/listobject.c
/// list_richcompare` (byte-for-byte the same algorithm).
unsafe fn seq_structural_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
    len: unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t,
    get: unsafe extern "C" fn(*mut PyObject, Py_ssize_t) -> *mut PyObject,
) -> *mut PyObject {
    // CPython comparison opcodes (Include/object.h).
    const PY_LT: c_int = 0;
    const PY_LE: c_int = 1;
    const PY_EQ: c_int = 2;
    const PY_NE: c_int = 3;
    const PY_GT: c_int = 4;
    const PY_GE: c_int = 5;

    let vlen = unsafe { len(v) };
    let wlen = unsafe { len(w) };
    if vlen < 0 || wlen < 0 {
        // The size accessor already set the exception.
        return ptr::null_mut();
    }

    // First index where items differ under Py_EQ.
    let mut i: Py_ssize_t = 0;
    while i < vlen && i < wlen {
        let vi = unsafe { get(v, i) };
        let wi = unsafe { get(w, i) };
        if vi.is_null() || wi.is_null() {
            return ptr::null_mut();
        }
        let k = unsafe { crate::api::typeobj::PyObject_RichCompareBool(vi, wi, PY_EQ) };
        if k < 0 {
            return ptr::null_mut();
        }
        if k == 0 {
            break;
        }
        i += 1;
    }

    // Ran off the end of one/both: the result is decided by the lengths.
    if i >= vlen || i >= wlen {
        let cmp = match op {
            PY_LT => vlen < wlen,
            PY_LE => vlen <= wlen,
            PY_EQ => vlen == wlen,
            PY_NE => vlen != wlen,
            PY_GT => vlen > wlen,
            PY_GE => vlen >= wlen,
            _ => return ptr::null_mut(),
        };
        return seq_richcmp_bool(cmp);
    }

    // A differing item exists — EQ/NE short-circuit without re-comparing.
    if op == PY_EQ {
        return seq_richcmp_bool(false);
    }
    if op == PY_NE {
        return seq_richcmp_bool(true);
    }
    // Ordering: compare the first differing item with the requested operator.
    let vi = unsafe { get(v, i) };
    let wi = unsafe { get(w, i) };
    if vi.is_null() || wi.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::typeobj::PyObject_RichCompare(vi, wi, op) }
}

/// Defer to `NotImplemented` (a new reference), CPython's contract when the two
/// operands are not the same built-in sequence type.
#[inline]
unsafe fn richcompare_not_implemented() -> *mut PyObject {
    let ni = &raw mut crate::abi_types::Py_NotImplementedSentinel;
    unsafe { crate::api::refcount::Py_INCREF(ni) };
    ni
}

/// CPython `Objects/tupleobject.c` `tuplerichcompare` — element-wise structural
/// comparison for tuples.
///
/// Without this slot, `do_richcompare` on two *distinct* tuple objects finds no
/// `tp_richcompare` and falls back to **object identity**, so `(a,a,a) == (a,a,a)`
/// over two distinct tuples is wrongly `False`. numpy's ufunc dispatch depends on
/// exactly this equality: `get_info_no_cast` (`_core/src/umath/dispatching.c`)
/// matches a registered loop with
/// `PyObject_RichCompareBool(cur_DType_tuple, t_dtypes, Py_EQ)`, where the two
/// tuples are always distinct objects holding equal `DTypeMeta` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_tuple_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    // Only tuple-vs-tuple; otherwise defer with NotImplemented (CPython behavior).
    if unsafe { PyTuple_Check(v) } == 0 || unsafe { PyTuple_Check(w) } == 0 {
        return unsafe { richcompare_not_implemented() };
    }
    unsafe { seq_structural_richcompare(v, w, op, PyTuple_Size, PyTuple_GetItem) }
}

/// CPython `Objects/listobject.c` `list_richcompare` — element-wise structural
/// comparison for lists (same algorithm as tuples). Sibling of the tuple slot:
/// without it, two *distinct* equal lists compare unequal by object identity in
/// `do_richcompare` (the zeroed-shell defect the coordinator flagged for the
/// container siblings). Reads through the dual-path `PyList_Size`/`PyList_GetItem`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_list_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    // Only list-vs-list; otherwise defer with NotImplemented (CPython behavior).
    if unsafe { PyList_Check(v) } == 0 || unsafe { PyList_Check(w) } == 0 {
        return unsafe { richcompare_not_implemented() };
    }
    unsafe { seq_structural_richcompare(v, w, op, PyList_Size, PyList_GetItem) }
}

#[allow(dead_code)]
unsafe fn py_tuple_pack_placeholder_removed_from_abi(n: Py_ssize_t, /* ... */) -> *mut PyObject {
    // Variadic — without va_list we can only create an empty tuple.
    // Real variadic support is in the C shim.
    unsafe { PyTuple_New(n) }
}

// ─── PySet ────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Check(op: *mut PyObject) -> c_int {
    // CPython Include/setobject.h: PySet_Check is set + set SUBTYPES ONLY —
    // frozenset is NOT included (the union is the PyAnySet_Check header macro,
    // which composes `PySet_Check || PyFrozenSet_Check` and therefore stays
    // correct with this split). The previous body unioned frozenset in.
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return 0;
    }
    if std::ptr::eq(ob_type, &raw const crate::abi_types::PySet_Type) {
        return 1;
    }
    unsafe { crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PySet_Type) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrozenSet_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return 0;
    }
    if std::ptr::eq(ob_type, &raw const crate::abi_types::PyFrozenSet_Type) {
        return 1;
    }
    // frozenset subtypes count too (Py_IS_TYPE || PyType_IsSubtype in CPython).
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyFrozenSet_Type)
    }
}

/// Ensure a NULL / -1 error return always carries a pending exception.
///
/// The runtime set hooks return the CPython error sentinel (0 / -1) after
/// setting a pending runtime exception. If a caller reaches a failure path with
/// no pending exception (e.g. hooks unregistered, or a bridge resolution
/// failure that the runtime never saw), the ABI contract still requires an
/// exception on the error return — set a SystemError so callers never observe a
/// NULL/-1 without `PyErr_Occurred()`.
unsafe fn ensure_set_error(message: &'static core::ffi::CStr) {
    let pending = crate::hooks::hooks()
        .map(|h| unsafe { (h.exception_pending)() } != 0)
        .unwrap_or(false);
    if !pending {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                message.as_ptr(),
            );
        }
    }
}

/// Resolve a bridge-managed set argument to its runtime handle bits, or set a
/// SystemError and return None (matching CPython's SystemError for a non-set).
unsafe fn set_arg_handle(anyset: *mut PyObject, message: &'static core::ffi::CStr) -> Option<u64> {
    // Drop the bridge lock before any PyErr_SetString / hook call — both
    // re-acquire GLOBAL_BRIDGE, so holding the guard would self-deadlock.
    let handle = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.pyobj_to_handle(anyset)
    };
    match handle {
        Some(bits) => Some(bits),
        None => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    message.as_ptr(),
                );
            }
            None
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_New(iterable: *mut PyObject) -> *mut PyObject {
    // NULL iterable → empty set (CPython accepts NULL). A non-NULL iterable must
    // be a bridge-managed object; resolve it to handle bits (0 signals "empty"
    // to the runtime authority).
    let iterable_bits = if iterable.is_null() {
        0
    } else {
        match unsafe {
            set_arg_handle(
                iterable,
                c"PySet_New: iterable is not a bridge-managed object",
            )
        } {
            Some(bits) => bits,
            None => return ptr::null_mut(),
        }
    };
    let h = hooks_or_stubs();
    let result = unsafe { (h.set_new)(iterable_bits) };
    if result == 0 {
        unsafe { ensure_set_error(c"PySet_New failed: runtime set authority unavailable") };
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Size(anyset: *mut PyObject) -> Py_ssize_t {
    let bits = match unsafe {
        set_arg_handle(anyset, c"PySet_Size: argument is not a bridge-managed set")
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let n = unsafe { (h.set_size)(bits) };
    if n < 0 {
        unsafe { ensure_set_error(c"PySet_Size failed: runtime set authority unavailable") };
        return -1;
    }
    n as Py_ssize_t
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Contains(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PySet_Contains: key must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    let set_bits = match unsafe {
        set_arg_handle(
            anyset,
            c"PySet_Contains: argument is not a bridge-managed set",
        )
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let key_bits =
        match unsafe { set_arg_handle(key, c"PySet_Contains: key is not a bridge-managed object") }
        {
            Some(bits) => bits,
            None => return -1,
        };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.set_contains)(set_bits, key_bits) };
    if rc < 0 {
        unsafe { ensure_set_error(c"PySet_Contains failed: runtime set authority unavailable") };
        return -1;
    }
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Add(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PySet_Add: key must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    let mutable_set = unsafe { PySet_Check(anyset) } != 0;
    let unique_exact_frozen = unsafe {
        !anyset.is_null()
            && std::ptr::eq(
                (*anyset).ob_type,
                &raw mut crate::abi_types::PyFrozenSet_Type,
            )
            && (*anyset).ob_refcnt == 1
    };
    if !mutable_set && !unique_exact_frozen {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let set_bits =
        match unsafe { set_arg_handle(anyset, c"PySet_Add: argument is not a bridge-managed set") }
        {
            Some(bits) => bits,
            None => return -1,
        };
    let key_bits =
        match unsafe { set_arg_handle(key, c"PySet_Add: key is not a bridge-managed object") } {
            Some(bits) => bits,
            None => return -1,
        };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.set_add)(set_bits, key_bits) };
    if rc != 0 {
        unsafe { ensure_set_error(c"PySet_Add failed: runtime set authority unavailable") };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrozenSet_New(iterable: *mut PyObject) -> *mut PyObject {
    let iterable_bits = if iterable.is_null() {
        0
    } else {
        match unsafe {
            set_arg_handle(iterable, c"PyFrozenSet_New: iterable is not bridge-managed")
        } {
            Some(bits) => bits,
            None => return ptr::null_mut(),
        }
    };
    let h = hooks_or_stubs();
    let result = unsafe { (h.set_op)(crate::hooks::SetOp::FrozenNew as u32, iterable_bits) };
    if result == 0 {
        unsafe { ensure_set_error(c"PyFrozenSet_New failed") };
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Pop(anyset: *mut PyObject) -> *mut PyObject {
    if unsafe { PySet_Check(anyset) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(bits) = (unsafe { set_arg_handle(anyset, c"PySet_Pop: invalid set") }) else {
        return ptr::null_mut();
    };
    let result = unsafe { (hooks_or_stubs().set_op)(crate::hooks::SetOp::Pop as u32, bits) };
    if result == 0 {
        unsafe { ensure_set_error(c"PySet_Pop failed") };
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Clear(anyset: *mut PyObject) -> c_int {
    if unsafe { PySet_Check(anyset) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let Some(bits) = (unsafe { set_arg_handle(anyset, c"PySet_Clear: invalid set") }) else {
        return -1;
    };
    let result = unsafe { (hooks_or_stubs().set_op)(crate::hooks::SetOp::Clear as u32, bits) };
    if result == 0 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Discard(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PySet_Discard: key must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    let set_bits = match unsafe {
        set_arg_handle(
            anyset,
            c"PySet_Discard: argument is not a bridge-managed set",
        )
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let key_bits = match unsafe {
        set_arg_handle(key, c"PySet_Discard: key is not a bridge-managed object")
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.set_discard)(set_bits, key_bits) };
    if rc < 0 {
        unsafe { ensure_set_error(c"PySet_Discard failed: runtime set authority unavailable") };
        return -1;
    }
    rc
}

// ─── PyList_GetSlice / PyList_Sort / PyList_Reverse ──────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetSlice(
    op: *mut PyObject,
    ilow: Py_ssize_t,
    ihigh: Py_ssize_t,
) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let len = unsafe { (h.list_len)(bits) } as Py_ssize_t;
    let low = ilow.max(0).min(len);
    let high = ihigh.max(low).min(len);
    let new_list = unsafe { (h.alloc_list)() };
    if new_list == 0 {
        return ptr::null_mut();
    }
    for i in low..high {
        let item = unsafe { (h.list_item)(bits, i as usize) };
        unsafe { (h.list_append)(new_list, item) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) }
}

/// Real slice assignment/deletion (CPython `list_ass_slice`): replaces
/// `op[ilow:ihigh]` with the elements of `itemlist` (a list/tuple), or deletes
/// the slice when `itemlist` is NULL. The previous body was a success-returning
/// NO-OP — every `del a[i:j]` / slice-replace silently did nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SetSlice(
    op: *mut PyObject,
    ilow: Py_ssize_t,
    ihigh: Py_ssize_t,
    itemlist: *mut PyObject,
) -> c_int {
    let list_bits = match resolve_native_list(op) {
        Some(b) => b,
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return -1;
        }
    };
    let itemlist_bits = if itemlist.is_null() {
        0 // deletion
    } else {
        let itemlist_handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(itemlist);
        match itemlist_handle {
            Some(b) if MoltObject::from_bits(b).is_ptr() => b,
            _ => {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return -1;
            }
        }
    };
    let h = hooks_or_stubs();
    // CPython INCREFs each item copied in from itemlist (the receiving list
    // takes its own references). Anchor the incoming item proxies exactly as
    // PyList_Append does; removed items keep their anchor (the documented
    // PyDict_SetItem custody trade — leak-not-corrupt).
    if itemlist_bits != 0 {
        let n = unsafe { (h.list_len)(itemlist_bits) };
        let mut bridge = GLOBAL_BRIDGE.lock();
        for idx in 0..n {
            let item_bits = unsafe { (h.list_item)(itemlist_bits, idx) };
            if item_bits != 0 && MoltObject::from_bits(item_bits).is_ptr() {
                let proxy = unsafe { bridge.handle_to_borrowed_pyobj(item_bits) };
                unsafe { crate::api::refcount::Py_INCREF(proxy) };
            }
        }
    }
    let rc = unsafe { (h.list_set_slice)(list_bits, ilow, ihigh, itemlist_bits) };
    if rc != 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Sort(op: *mut PyObject) -> c_int {
    // CPython: BadInternalCall on a non-list; sorts IN PLACE via the runtime
    // comparison authority. The previous body discarded op and fabricated
    // success (0) without sorting.
    let list_bits = match resolve_native_list(op) {
        Some(b) => b,
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return -1;
        }
    };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.list_sort)(list_bits) };
    if rc != 0 {
        // The runtime raises for uncomparable elements; guarantee an exception
        // on the sentinel either way (ABI contract).
        unsafe { ensure_set_error(c"PyList_Sort failed: runtime sort authority unavailable") };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Reverse(op: *mut PyObject) -> c_int {
    // CPython: BadInternalCall on a non-list; reverses IN PLACE. The previous
    // body discarded op and fabricated success (0) without reversing.
    let list_bits = match resolve_native_list(op) {
        Some(b) => b,
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return -1;
        }
    };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.list_reverse)(list_bits) };
    if rc != 0 {
        unsafe { ensure_set_error(c"PyList_Reverse failed: runtime list authority unavailable") };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_AsTuple(op: *mut PyObject) -> *mut PyObject {
    if unsafe { PyList_Check(op) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let len = unsafe { PyList_Size(op) };
    let new_tuple = unsafe { PyTuple_New(len) };
    if new_tuple.is_null() {
        return ptr::null_mut();
    }
    for i in 0..len {
        let item = unsafe { PyList_GetItem(op, i) };
        if item.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_INCREF(item) };
        if unsafe { PyTuple_SetItem(new_tuple, i, item) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
            return ptr::null_mut();
        }
    }
    new_tuple
}

// ─── PyList_Insert ───────────────────────────────────────────────────────

/// Real indexed insert (CPython `ins1`): inserts `v` BEFORE the (negative-
/// adjusted, clamped) index `where_`, shifting subsequent elements right. Does
/// NOT steal `v` (CPython `Py_NewRef(v)`) — the item proxy is anchored exactly
/// like `PyList_Append`. The previous body always APPENDED, mis-ordering every
/// `where_ < len` insert.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Insert(
    op: *mut PyObject,
    where_: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    if v.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let list_bits = match resolve_native_list(op) {
        Some(b) => b,
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return -1;
        }
    };
    let mut bridge = GLOBAL_BRIDGE.lock();
    let mut item_is_foreign = false;
    let item_bits = match bridge.pyobj_to_handle(v) {
        Some(b) => b,
        None => match unsafe { bridge.molt_value_for_pyobj(v) } {
            Some(b) => {
                item_is_foreign = true;
                b
            }
            None => {
                drop(bridge);
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_SystemError,
                            c"PyList_Insert: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let rc = unsafe { (h.list_insert)(list_bits, where_, item_bits) };
    if rc != 0 {
        unsafe { ensure_set_error(c"PyList_Insert failed: runtime list authority unavailable") };
        return -1;
    }
    // Non-stealing contract (same anchor discipline as PyList_Append). A
    // foreign wrapper already owns its strong reference.
    if !item_is_foreign {
        unsafe { crate::api::refcount::Py_INCREF(v) };
    }
    0
}
