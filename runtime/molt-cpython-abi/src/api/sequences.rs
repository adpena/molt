//! Sequence API — PyList_*, PyTuple_*.

use crate::abi_types::{Py_ssize_t, PyListObject, PyObject, PyTupleObject};
#[allow(unused_imports)]
use crate::abi_types::{PyList_Type, PyTuple_Type};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use std::os::raw::c_int;
use std::ptr;

// ─── PyList ───────────────────────────────────────────────────────────────

/// Resolve `op` to its runtime handle bits iff it is a Molt-native list.
/// `None` → the caller sets the CPython-shaped exception (`BadInternalCall`).
fn resolve_native_list(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    let bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(op)?;
    if !bits.decode().is_ptr() {
        return None;
    }
    let h = hooks_or_stubs();
    if unsafe { (h.classify_heap)(bits.bits()) } == crate::abi_types::MoltTypeTag::List as u8 {
        Some(bits.bits())
    } else {
        None
    }
}

/// A stable list-layout read transaction. Both bridge-managed lists and ABI
/// list subclasses expose the truthful `PyListObject` prefix. Managed views
/// are committed once, and the runtime GIL remains pinned for the transaction;
/// this is the single boundary to evolve into per-object critical sections for
/// a free-threaded runtime.
pub(crate) struct ListRead {
    object: *mut PyListObject,
    _runtime_gil: crate::hooks::RuntimeGilGuard,
}

impl ListRead {
    pub(crate) unsafe fn acquire(op: *mut PyObject) -> Option<Self> {
        unsafe { Self::acquire_with_completeness(op, true) }
    }

    unsafe fn acquire_len(op: *mut PyObject) -> Option<Self> {
        unsafe { Self::acquire_with_completeness(op, false) }
    }

    unsafe fn acquire_with_completeness(op: *mut PyObject, require_complete: bool) -> Option<Self> {
        if op.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        let runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        if let Some(bits) = resolve_native_list(op) {
            let committed = if require_complete {
                GLOBAL_BRIDGE.commit_list_view(bits)
            } else {
                GLOBAL_BRIDGE.commit_list_view_partial(bits)
            };
            if !committed {
                return None;
            }
        } else if unsafe { PyList_Check(op) } == 0 {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        let object = op.cast::<PyListObject>();
        let len = unsafe { (*object).ob_base.ob_size };
        if len < 0 || (len > 0 && unsafe { (*object).ob_item }.is_null()) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"invalid PyListObject layout".as_ptr(),
                );
            }
            return None;
        }
        Some(Self {
            object,
            _runtime_gil: runtime_gil,
        })
    }

    #[inline]
    pub(crate) unsafe fn len(&self) -> usize {
        unsafe { (*self.object).ob_base.ob_size as usize }
    }

    #[inline]
    pub(crate) unsafe fn item(&self, index: usize) -> *mut PyObject {
        let len = unsafe { self.len() };
        if index >= len {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_IndexError).cast::<PyObject>(),
                    c"list index out of range".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        let item = unsafe { *(*self.object).ob_item.add(index) };
        if item.is_null() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"PyListObject contains an uninitialized item".as_ptr(),
                );
            }
        }
        item
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
    let bits = unsafe { (h.alloc_list_presized)(size as usize) };
    if bits == 0 {
        // Allocation failed. CPython's PyList_New returns NULL with MemoryError
        // set. Returning Py_None (non-NULL) here would defeat the extension's
        // `if (list == NULL)` guard and let it operate on None as if it were a
        // list — silent corruption. Fail closed with NULL + a set exception.
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_MemoryError).cast::<crate::abi_types::PyObject>(),
                c"PyList_New: failed to allocate list".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    // CPython pre-sizes the list to `size` NULL slots: PyList_GET_SIZE reports
    // `size` immediately and PyList_SetItem/SET_ITEM stores at any index in
    // [0,size), including out of order. Molt establishes the logical length in
    // one backing-store allocation and the bridge records that the physical
    // slots remain unreadable until the extension initializes them.
    let result = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    if result.is_null() || !GLOBAL_BRIDGE.mark_list_view_uninitialized(bits) {
        unsafe { crate::api::refcount::Py_XDECREF(result) };
        return ptr::null_mut();
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Append(list: *mut PyObject, item: *mut PyObject) -> c_int {
    // CPython: non-list or NULL newitem → PyErr_BadInternalCall() + -1. Every
    // -1 return must carry a pending exception (sentinel sweep).
    if list.is_null() || item.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let list_bits = match resolve_native_list(list) {
        Some(bits) => bits,
        None => {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return -1;
        }
    };
    if !bridge.commit_list_view(list_bits) {
        return -1;
    }
    let (item_bits, owned_local) = match bridge.molt_handle_for_pyobj(item) {
        Some(b) => (b.bits(), false),
        None => match unsafe { bridge.molt_value_for_pyobj(item) } {
            // A genuine C-extension object item: give it a first-class
            // `TYPE_ID_FOREIGN` wrapper so it can be stored in the Molt list.
            Some(b) => (b, true),
            None => {
                // No foreign wrapper could be minted (runtime hooks absent).
                // Fail loud instead of a contentless -1.
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                            c"PyList_Append: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.list_append)(list_bits, item_bits, item) };
    // CPython contract: `PyList_Append` takes its own strong reference to the
    // item (it does not steal). Anchor the item proxy so the extension's
    // balancing `Py_DECREF` cannot sever the pointer↔handle mapping while the
    // item stays reachable from the runtime list (same class as the
    // `PyDict_SetItem` anchor — see api/mapping.rs). A foreign-wrapped item
    // already holds its own strong reference on the C object (minted at
    // refcount 1, ownership transferred to the list), so it is not INCREF'd.
    if owned_local {
        unsafe { (h.dec_ref)(item_bits) };
    }
    if rc != 0 {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
        }
        return -1;
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
    let list = op.cast::<PyListObject>();
    let len = unsafe { (*list).ob_base.ob_size };
    if i >= len || unsafe { (*list).ob_item }.is_null() {
        return ptr::null_mut();
    }
    unsafe { *(*list).ob_item.add(i as usize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    // CPython: non-list → PyErr_BadInternalCall; OOB (incl. negative) →
    // IndexError "list index out of range"; success → borrowed ref. The prior
    // delegation to GET_ITEM returned bare NULLs with no exception on both
    // error classes (silent-sentinel row).
    let Some(read) = (unsafe { ListRead::acquire(op) }) else {
        return ptr::null_mut();
    };
    if i < 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_IndexError).cast::<PyObject>(),
                c"list index out of range".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe { read.item(i as usize) }
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
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if !GLOBAL_BRIDGE.commit_list_view_partial(list_bits) {
        unsafe { crate::api::refcount::Py_XDECREF(v) };
        return -1;
    }
    if v.is_null() {
        // CPython would store a NULL slot; Molt lists have no NULL slot. An
        // honest SystemError beats fabricating a stored value.
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let (val_bits, owned_local) = match bridge.molt_handle_for_pyobj(v) {
        Some(b) => (b.bits(), false),
        None => match unsafe { bridge.molt_value_for_pyobj(v) } {
            Some(b) => (b, true),
            None => {
                unsafe {
                    crate::api::refcount::Py_XDECREF(v);
                    if crate::api::errors::PyErr_Occurred().is_null() {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                            c"PyList_SetItem: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    if !GLOBAL_BRIDGE.prepare_list_set_stolen_ref(v) {
        if owned_local {
            unsafe { (hooks_or_stubs().dec_ref)(val_bits) };
        }
        unsafe { crate::api::refcount::Py_XDECREF(v) };
        return -1;
    }
    let h = hooks_or_stubs();
    let stored = if i >= 0 {
        unsafe { (h.list_set)(list_bits, i as usize, val_bits) }
    } else {
        crate::hooks::OwnedHandleResult::error()
    };
    let old_bits = match stored.decode() {
        crate::hooks::DecodedHandleResult::Ok(bits) => Some(bits),
        crate::hooks::DecodedHandleResult::Missing | crate::hooks::DecodedHandleResult::Error => {
            None
        }
    };
    let Some(old_bits) = old_bits else {
        GLOBAL_BRIDGE.cancel_list_set_stolen_ref(v);
        if owned_local {
            unsafe { (h.dec_ref)(val_bits) };
        }
        unsafe { crate::api::refcount::Py_XDECREF(v) };
        // OOB: CPython Py_XDECREFs the stolen reference, then IndexError.
        unsafe {
            if crate::api::errors::PyErr_Occurred().is_null() {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_IndexError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"list assignment index out of range".as_ptr(),
                );
            }
        }
        return -1;
    };
    if !GLOBAL_BRIDGE.publish_list_set_from_stolen(list_bits, i as usize, v) {
        eprintln!("molt fatal: successful runtime list store could not publish its ABI view");
        std::process::abort();
    }
    if owned_local {
        unsafe { (h.dec_ref)(val_bits) };
    }
    unsafe { (h.dec_ref)(old_bits) };
    // Steal contract on success: the container takes over the caller's
    // reference — a bridge proxy is NOT INCREF'd (unlike the non-stealing
    // Append), and a foreign value's stolen C reference is consumed now (the
    // TYPE_ID_FOREIGN wrapper holds its own strong reference for custody).
    0
}

/// Exact-size list publication transaction. The builder owns the destination
/// until every slot is initialized, so every failure retires the partial list
/// exactly once.
struct ExactListBuilder {
    list: *mut PyObject,
    next: Py_ssize_t,
    size: Py_ssize_t,
}

impl ExactListBuilder {
    unsafe fn new(size: Py_ssize_t) -> Option<Self> {
        let list = unsafe { PyList_New(size) };
        (!list.is_null()).then_some(Self {
            list,
            next: 0,
            size,
        })
    }

    unsafe fn push_owned(&mut self, item: *mut PyObject) -> bool {
        if item.is_null() || self.next >= self.size {
            unsafe { crate::api::refcount::Py_XDECREF(item) };
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"invalid exact-list publication".as_ptr(),
                    );
                }
            }
            return false;
        }
        if unsafe { PyList_SetItem(self.list, self.next, item) } != 0 {
            return false;
        }
        self.next += 1;
        true
    }

    unsafe fn push_borrowed(&mut self, item: *mut PyObject) -> bool {
        if item.is_null() {
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"NULL borrowed item in exact-list publication".as_ptr(),
                    );
                }
            }
            return false;
        }
        unsafe { crate::api::refcount::Py_INCREF(item) };
        unsafe { self.push_owned(item) }
    }

    unsafe fn finish(mut self) -> *mut PyObject {
        if self.next != self.size {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"incomplete exact-list publication".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        std::mem::replace(&mut self.list, ptr::null_mut())
    }
}

impl Drop for ExactListBuilder {
    fn drop(&mut self) {
        unsafe { crate::api::refcount::Py_XDECREF(self.list) };
    }
}

pub(crate) unsafe fn list_from_borrowed_indexed<F>(
    size: Py_ssize_t,
    mut item_at: F,
) -> *mut PyObject
where
    F: FnMut(Py_ssize_t) -> *mut PyObject,
{
    let Some(mut builder) = (unsafe { ExactListBuilder::new(size) }) else {
        return ptr::null_mut();
    };
    for index in 0..size {
        if !unsafe { builder.push_borrowed(item_at(index)) } {
            return ptr::null_mut();
        }
    }
    unsafe { builder.finish() }
}

pub(crate) unsafe fn list_from_owned_indexed<F>(size: Py_ssize_t, mut item_at: F) -> *mut PyObject
where
    F: FnMut(Py_ssize_t) -> *mut PyObject,
{
    let Some(mut builder) = (unsafe { ExactListBuilder::new(size) }) else {
        return ptr::null_mut();
    };
    for index in 0..size {
        if !unsafe { builder.push_owned(item_at(index)) } {
            return ptr::null_mut();
        }
    }
    unsafe { builder.finish() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return 0;
    }
    unsafe { (*op.cast::<PyListObject>()).ob_base.ob_size }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Size(op: *mut PyObject) -> Py_ssize_t {
    // CPython: `if (!PyList_Check(op)) { PyErr_BadInternalCall(); return -1; }`.
    let Some(read) = (unsafe { ListRead::acquire_len(op) }) else {
        return -1;
    };
    unsafe { read.len() as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if resolve_native_list(op).is_some() {
        return 1;
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_CheckExact(op: *mut PyObject) -> c_int {
    if resolve_native_list(op).is_some() {
        return 1;
    }
    (!op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PyList_Type,
        )) as c_int
}

// ─── PyTuple ──────────────────────────────────────────────────────────────

pub(crate) unsafe fn tuple_layout_object(op: *mut PyObject) -> Option<*mut PyTupleObject> {
    if op.is_null() {
        return None;
    }
    let ob_type = unsafe { (*op).ob_type };
    if !ob_type.is_null()
        && (std::ptr::eq(ob_type, &raw mut crate::abi_types::PyTuple_Type)
            || unsafe {
                crate::api::typeobj::PyType_IsSubtype(
                    ob_type,
                    &raw mut crate::abi_types::PyTuple_Type,
                )
            } != 0)
    {
        Some(op.cast::<PyTupleObject>())
    } else {
        None
    }
}

#[inline]
pub(crate) unsafe fn tuple_items_ptr(tuple: *mut PyTupleObject) -> *mut *mut PyObject {
    unsafe { std::ptr::addr_of_mut!((*tuple).ob_item).cast() }
}

unsafe fn native_tuple_new(size: Py_ssize_t) -> *mut PyObject {
    unsafe { crate::api::memory::molt_object_alloc(&raw mut crate::abi_types::PyTuple_Type, size) }
}

/// Build the physical positional-argument tuple used by native C call slots.
/// This authority is independent of managed-runtime tuple hooks, which is
/// essential while reporting an error from a missing or failed hook itself.
/// Items are borrowed and the returned exact tuple owns one reference each.
pub(crate) unsafe fn native_call_args(items: &[*mut PyObject]) -> *mut PyObject {
    let Ok(size) = Py_ssize_t::try_from(items.len()) else {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return ptr::null_mut();
    };
    let tuple = unsafe { native_tuple_new(size) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    let tuple_layout = tuple.cast::<PyTupleObject>();
    for (index, &item) in items.iter().enumerate() {
        if item.is_null() {
            unsafe {
                crate::api::refcount::Py_DECREF(tuple);
                crate::api::errors::PyErr_BadInternalCall();
            }
            return ptr::null_mut();
        }
        unsafe {
            crate::api::refcount::Py_INCREF(item);
            *tuple_items_ptr(tuple_layout).add(index) = item;
        }
    }
    tuple
}

/// Deallocate a genuine foreign tuple subclass. Exact builtin tuples are
/// runtime-backed managed views and are retired by the bridge before `tp_dealloc`
/// dispatch; only subclass storage allocated by its type reaches this function.
pub unsafe extern "C" fn molt_tuple_subtype_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let tuple = op.cast::<PyTupleObject>();
    let len = unsafe { (*tuple).ob_base.ob_size.max(0) as usize };
    let items = unsafe { std::slice::from_raw_parts_mut(tuple_items_ptr(tuple), len) };
    for pointer in items.iter_mut() {
        let item = std::mem::replace(pointer, ptr::null_mut());
        unsafe { crate::api::refcount::Py_XDECREF(item) };
    }
    let ty = unsafe { (*op).ob_type };
    if !ty.is_null()
        && let Some(free) = unsafe { (*ty).tp_free }
    {
        unsafe { free(op.cast()) };
    } else {
        unsafe { crate::api::memory::PyObject_GC_Del(op.cast()) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_New(size: Py_ssize_t) -> *mut PyObject {
    if size < 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if !crate::hooks::managed_tuple_construction_available() {
        return unsafe { native_tuple_new(size) };
    }
    let bits = unsafe { (hooks_or_stubs().alloc_tuple)(size as usize) };
    if bits == 0 {
        if !crate::api::errors::transfer_runtime_pending_to_current()
            && unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
        {
            unsafe { crate::api::errors::PyErr_NoMemory() };
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
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
        if i >= len {
            return ptr::null_mut();
        }
        return unsafe { *tuple_items_ptr(tuple).add(i as usize) };
    }
    let bridge = &*GLOBAL_BRIDGE;
    let bits = match bridge.molt_handle_for_pyobj(op) {
        Some(b) => b.bits(),
        None => return ptr::null_mut(),
    };
    let h = hooks_or_stubs();
    unsafe { GLOBAL_BRIDGE.borrowed_result_to_borrowed_pyobj((h.tuple_item)(bits, i as usize)) }
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
        if i < 0 || i >= len {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_IndexError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"tuple index out of range".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        return unsafe { *tuple_items_ptr(tuple).add(i as usize) };
    }
    // Bridge-managed Molt tuple.
    let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
    let bits = match op_handle {
        Some(b) if b.decode().is_ptr() => b.bits(),
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
        let result = unsafe { (h.tuple_item)(bits, i as usize) };
        if let crate::hooks::DecodedHandleResult::Ok(bits) = result.decode() {
            return unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
        }
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_IndexError).cast::<crate::abi_types::PyObject>(),
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
    let bridge = &*GLOBAL_BRIDGE;
    let bits = match bridge.molt_handle_for_pyobj(op) {
        Some(b) => b.bits(),
        None => return 0,
    };
    let h = hooks_or_stubs();
    unsafe { (h.tuple_len)(bits) as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Size(op: *mut PyObject) -> Py_ssize_t {
    if unsafe { PyTuple_Check(op) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
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
    if lo == 0 && hi == len && unsafe { PyTuple_CheckExact(op) } != 0 {
        unsafe { crate::api::refcount::Py_INCREF(op) };
        return op;
    }
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
            unsafe { crate::api::refcount::Py_DECREF(out) };
            return ptr::null_mut();
        }
    }
    out
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyTuple_Resize(pv: *mut *mut PyObject, newsize: Py_ssize_t) -> c_int {
    if pv.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let op = unsafe { *pv };
    if newsize < 0 || unsafe { PyTuple_CheckExact(op) } == 0 || unsafe { (*op).ob_refcnt } != 1 {
        unsafe {
            *pv = ptr::null_mut();
            crate::api::refcount::Py_XDECREF(op);
            crate::api::errors::PyErr_BadInternalCall();
        }
        return -1;
    }
    let oldsize = unsafe { PyTuple_GET_SIZE(op) };
    if oldsize == newsize {
        return 0;
    }
    let replacement = unsafe { PyTuple_New(newsize) };
    if replacement.is_null() {
        unsafe {
            *pv = ptr::null_mut();
            crate::api::refcount::Py_DECREF(op);
        }
        return -1;
    }
    let copied = oldsize.min(newsize);
    for index in 0..copied {
        let item = unsafe { PyTuple_GET_ITEM(op, index) };
        if item.is_null() {
            continue;
        }
        unsafe { crate::api::refcount::Py_INCREF(item) };
        if unsafe { PyTuple_SetItem(replacement, index, item) } != 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(replacement);
                crate::api::refcount::Py_DECREF(op);
                *pv = ptr::null_mut();
            }
            return -1;
        }
    }
    unsafe {
        crate::api::refcount::Py_DECREF(op);
        *pv = replacement;
    }
    0
}

/// Faithful `Objects/tupleobject.c` `PyTuple_SetItem`: steals the reference to
/// `v` and `Py_XDECREF`s it on EVERY error path; non-tuple → BadInternalCall;
/// OOB → IndexError "tuple assignment index out of range". The bridge and
/// fixed-length runtime storage both reject an OOB index. Foreign `v` gets
/// `TYPE_ID_FOREIGN` custody (same contract as `PyList_SetItem`).
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
    // CPython permits tuple slot initialization only while the new exact
    // tuple has unique ownership. Enforce that publication boundary before
    // either physical or runtime-backed storage can change; otherwise a
    // shared tuple could be mutated behind readers that correctly treat it as
    // immutable (and behind future free-threaded readers).
    if unsafe { (*op).ob_refcnt } != 1 {
        unsafe {
            crate::api::refcount::Py_XDECREF(v);
            crate::api::errors::PyErr_BadInternalCall();
        }
        return -1;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let managed_bits = bridge.molt_handle_for_pyobj(op).map(|value| value.bits());
    // Genuine foreign tuple subclasses retain their physical storage. Exact
    // builtin tuples are always bridge-managed from PyTuple_New onward.
    if managed_bits.is_none()
        && let Some(tuple) = unsafe { tuple_layout_object(op) }
    {
        let len = unsafe { (*tuple).ob_base.ob_size };
        if i < 0 || i >= len {
            unsafe {
                crate::api::refcount::Py_XDECREF(v);
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_IndexError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"tuple assignment index out of range".as_ptr(),
                );
            }
            return -1;
        }
        let slot = unsafe { tuple_items_ptr(tuple).add(i as usize) };
        unsafe {
            let old = *slot;
            *slot = v;
            crate::api::refcount::Py_XDECREF(old);
        }
        return 0;
    }
    let tuple_bits = match managed_bits {
        Some(bits) => bits,
        _ => {
            unsafe {
                crate::api::refcount::Py_XDECREF(v);
                crate::api::errors::PyErr_BadInternalCall();
            }
            return -1;
        }
    };
    let h = hooks_or_stubs();
    if unsafe { (h.ref_count)(tuple_bits) } != 1 {
        unsafe {
            crate::api::refcount::Py_XDECREF(v);
            crate::api::errors::PyErr_BadInternalCall();
        }
        return -1;
    }
    let (val_bits, owned_local) = match bridge.observed_handle_for_pyobj(v) {
        Some(b) => (b.bits(), false),
        None => match unsafe { bridge.molt_value_for_pyobj(v) } {
            Some(b) => (b, true),
            None => {
                unsafe {
                    crate::api::refcount::Py_XDECREF(v);
                    if crate::api::errors::PyErr_Occurred().is_null() {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                            c"PyTuple_SetItem: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    let is_tuple =
        unsafe { (h.classify_heap)(tuple_bits) } == crate::abi_types::MoltTypeTag::Tuple as u8;
    let in_bounds = i >= 0 && is_tuple && (i as usize) < unsafe { (h.tuple_len)(tuple_bits) };
    if !in_bounds {
        if owned_local {
            unsafe { (h.dec_ref)(val_bits) };
        }
        unsafe {
            crate::api::refcount::Py_XDECREF(v);
            if is_tuple {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_IndexError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"tuple assignment index out of range".as_ptr(),
                );
            } else {
                crate::api::errors::PyErr_BadInternalCall();
            }
        }
        return -1;
    }
    let Some(prepared) =
        (unsafe { bridge.prepare_tuple_value(tuple_bits, i as usize, val_bits, v) })
    else {
        if owned_local {
            unsafe { (h.dec_ref)(val_bits) };
        }
        return -1;
    };
    let result = unsafe { (h.tuple_set)(tuple_bits, i as usize, val_bits, v) };
    let old_bits = match result.decode() {
        crate::hooks::DecodedHandleResult::Ok(bits) => Some(bits),
        crate::hooks::DecodedHandleResult::Missing => Some(0),
        crate::hooks::DecodedHandleResult::Error => None,
    };
    if owned_local {
        unsafe { (h.dec_ref)(val_bits) };
    }
    let Some(old_bits) = old_bits else {
        // `prepared` consumes the stolen C reference on this error path.
        drop(prepared);
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
        }
        return -1;
    };
    let Some(retired) = (unsafe { prepared.publish() }) else {
        eprintln!("molt fatal: committed tuple mutation could not publish canonical ABI slot");
        std::process::abort();
    };
    // Publish both authorities before either displaced edge can run a
    // finalizer. Physical ownership retires first, then the runtime edge.
    drop(retired);
    if old_bits != 0 {
        unsafe { (h.dec_ref)(old_bits) };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op)
        && value.decode().is_ptr()
        && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
            == crate::abi_types::MoltTypeTag::Tuple as u8
    {
        return 1;
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_CheckExact(op: *mut PyObject) -> c_int {
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        return (value.decode().is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::Tuple as u8) as c_int;
    }
    (!op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PyTuple_Type,
        )) as c_int
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
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        return (value.decode().is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::Set as u8) as c_int;
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
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        return (value.decode().is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::FrozenSet as u8) as c_int;
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_CheckExact(op: *mut PyObject) -> c_int {
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        return (value.decode().is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::Set as u8) as c_int;
    }
    (!op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PySet_Type,
        )) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrozenSet_CheckExact(op: *mut PyObject) -> c_int {
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        return (value.decode().is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::FrozenSet as u8) as c_int;
    }
    (!op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PyFrozenSet_Type,
        )) as c_int
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
        let bridge = &*GLOBAL_BRIDGE;
        bridge.molt_handle_for_pyobj(anyset)
    };
    match handle {
        Some(bits) => Some(bits.bits()),
        None => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
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
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(result) }
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PySet_Add: key must not be NULL".as_ptr(),
            );
        }
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
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
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
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
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
    match result.decode() {
        crate::hooks::DecodedHandleResult::Ok(_) => 0,
        crate::hooks::DecodedHandleResult::Missing | crate::hooks::DecodedHandleResult::Error => {
            unsafe { ensure_set_error(c"PySet_Clear failed") };
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Discard(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
    let Some(read) = (unsafe { ListRead::acquire(op) }) else {
        return ptr::null_mut();
    };
    let len = unsafe { read.len() as Py_ssize_t };
    let low = ilow.max(0).min(len);
    let high = ihigh.max(low).min(len);
    unsafe { list_from_borrowed_indexed(high - low, |index| read.item((low + index) as usize)) }
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
    if !GLOBAL_BRIDGE.commit_list_view(list_bits) {
        return -1;
    }
    let itemlist_bits = if itemlist.is_null() {
        0 // deletion
    } else {
        let itemlist_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(itemlist);
        match itemlist_handle {
            Some(b) if b.decode().is_ptr() => {
                if unsafe { (hooks_or_stubs().classify_heap)(b.bits()) }
                    == crate::abi_types::MoltTypeTag::List as u8
                    && !GLOBAL_BRIDGE.commit_list_view(b.bits())
                {
                    return -1;
                }
                b.bits()
            }
            _ => {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return -1;
            }
        }
    };
    let Some(current) =
        (unsafe { crate::api::abstract_sequence::materialize_iterable_pointers(op, None) })
    else {
        return -1;
    };
    let replacement = if itemlist.is_null() {
        None
    } else {
        let Some(items) = (unsafe {
            crate::api::abstract_sequence::materialize_iterable_pointers(itemlist, None)
        }) else {
            return -1;
        };
        Some(items)
    };
    let len = current.len() as Py_ssize_t;
    let low = ilow.clamp(0, len) as usize;
    let high = ihigh.clamp(low as Py_ssize_t, len) as usize;
    let replacement_len = replacement.as_ref().map_or(0, |items| items.len());
    let Some(future_len) = current
        .len()
        .checked_sub(high - low)
        .and_then(|base| base.checked_add(replacement_len))
    else {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return -1;
    };
    let mut future = Vec::new();
    if future.try_reserve_exact(future_len).is_err() {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return -1;
    }
    future.extend_from_slice(&current.as_slice()[..low]);
    if let Some(replacement) = &replacement {
        future.extend_from_slice(replacement.as_slice());
    }
    future.extend_from_slice(&current.as_slice()[high..]);
    let h = hooks_or_stubs();
    let rc = unsafe {
        (h.list_set_slice)(
            list_bits,
            ilow,
            ihigh,
            itemlist_bits,
            future.as_ptr(),
            future.len(),
        )
    };
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
    if !GLOBAL_BRIDGE.commit_list_view(list_bits) {
        return -1;
    }
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
    if !GLOBAL_BRIDGE.commit_list_view(list_bits) {
        return -1;
    }
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
    if !GLOBAL_BRIDGE.commit_list_view(list_bits) {
        return -1;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let mut item_is_foreign = false;
    let item_bits = match bridge.molt_handle_for_pyobj(v) {
        Some(b) => b.bits(),
        None => match unsafe { bridge.molt_value_for_pyobj(v) } {
            Some(b) => {
                item_is_foreign = true;
                b
            }
            None => {
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                            c"PyList_Insert: item is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.list_insert)(list_bits, where_, item_bits, v) };
    if rc != 0 {
        if item_is_foreign {
            unsafe { (h.dec_ref)(item_bits) };
        }
        unsafe { ensure_set_error(c"PyList_Insert failed: runtime list authority unavailable") };
        return -1;
    }
    if item_is_foreign {
        unsafe { (h.dec_ref)(item_bits) };
    }
    0
}
