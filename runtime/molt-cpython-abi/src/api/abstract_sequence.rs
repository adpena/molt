//! Sequence abstract protocol — PySequence_* operations.
//!
//! Faithful to CPython 3.12 `Objects/abstract.c`: every function keeps a
//! zero-overhead Molt-native tier (list/tuple/str/bytes via the runtime hooks),
//! then a foreign type-slot tier (`tp_as_sequence` dispatch, mirroring the
//! `foreign_get_item` pattern in `api/object.rs`), then an iterator fallback
//! where CPython has one — and every error return carries the CPython-shaped
//! exception (the pre-sweep code returned bare `-1`/NULL sentinels and
//! fabricated empty results; see the divergence ledger rows for this file).

use crate::abi_types::{Py_ssize_t, PyListObject, PyObject, PySequenceMethods, PyTupleObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

/// CPython `Include/object.h` rich-comparison opcode `Py_EQ` (the flag
/// constants live only in the C header tier, per abi_types).
const PY_EQ: c_int = 2;

type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
type SsizeArgFunc = unsafe extern "C" fn(*mut PyObject, Py_ssize_t) -> *mut PyObject;
type BinaryFunc = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
type SsizeObjArgProc = unsafe extern "C" fn(*mut PyObject, Py_ssize_t, *mut PyObject) -> c_int;
type ObjObjProc = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> c_int;

#[derive(Clone, Copy)]
enum SequenceNumericMode {
    Regular,
    InPlace,
}

/// CPython's numeric-slot fallback for sequence concatenation. The numeric
/// dispatcher is shared with `PyNumber_*`, including reflected-subtype priority
/// and exact `NotImplemented` ownership. `None` means every slot declined.
unsafe fn sequence_add_numeric_fallback(
    mode: SequenceNumericMode,
    left: *mut PyObject,
    right: *mut PyObject,
) -> Option<*mut PyObject> {
    use crate::api::abstract_number::{BinarySlot, InPlaceSlot};
    let result = match mode {
        SequenceNumericMode::Regular => unsafe {
            crate::api::abstract_number::foreign_binary_op1(BinarySlot::Add, left, right)
        },
        SequenceNumericMode::InPlace => unsafe {
            crate::api::abstract_number::foreign_binary_iop1(
                InPlaceSlot::Add,
                BinarySlot::Add,
                left,
                right,
            )
        },
    };
    if crate::api::abstract_number::is_not_implemented(result) {
        unsafe { crate::api::abstract_number::discard_not_implemented(result) };
        None
    } else {
        Some(result)
    }
}

/// CPython's numeric-slot fallback for sequence repetition. The temporary
/// count carrier owns exactly one reference, and the returned result follows
/// the same `Some(NULL-with-error)` / `None-all-declined` contract as concat.
unsafe fn sequence_multiply_numeric_fallback(
    mode: SequenceNumericMode,
    object: *mut PyObject,
    count: Py_ssize_t,
) -> Option<*mut PyObject> {
    use crate::api::abstract_number::{BinarySlot, InPlaceSlot};
    let count_object = unsafe { crate::api::numbers::PyLong_FromSsize_t(count) };
    if count_object.is_null() {
        return Some(ptr::null_mut());
    }
    let result = match mode {
        SequenceNumericMode::Regular => unsafe {
            crate::api::abstract_number::foreign_binary_op1(
                BinarySlot::Multiply,
                object,
                count_object,
            )
        },
        SequenceNumericMode::InPlace => unsafe {
            crate::api::abstract_number::foreign_binary_iop1(
                InPlaceSlot::Multiply,
                BinarySlot::Multiply,
                object,
                count_object,
            )
        },
    };
    unsafe { crate::api::refcount::Py_DECREF(count_object) };
    if crate::api::abstract_number::is_not_implemented(result) {
        unsafe { crate::api::abstract_number::discard_not_implemented(result) };
        None
    } else {
        Some(result)
    }
}

/// Helper: resolve a PyObject to its Molt bits.
fn resolve_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE
        .molt_handle_for_pyobj(op)
        .map(|value| value.bits())
}

/// Resolve a value that is about to be semantically observed by the runtime.
fn observed_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE
        .observed_handle_for_pyobj(op)
        .map(|value| value.bits())
}

/// Helper: classify a heap-pointer handle.
fn classify(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    if !obj.is_ptr() {
        return crate::abi_types::MoltTypeTag::Other as u8;
    }
    let h = hooks_or_stubs();
    unsafe { (h.classify_heap)(bits) }
}

#[inline]
fn tag_list() -> u8 {
    crate::abi_types::MoltTypeTag::List as u8
}
#[inline]
fn tag_tuple() -> u8 {
    crate::abi_types::MoltTypeTag::Tuple as u8
}
#[inline]
fn tag_str() -> u8 {
    crate::abi_types::MoltTypeTag::Str as u8
}
#[inline]
fn tag_bytes() -> u8 {
    crate::abi_types::MoltTypeTag::Bytes as u8
}
#[inline]
fn tag_dict() -> u8 {
    crate::abi_types::MoltTypeTag::Dict as u8
}

/// Set a `TypeError` with a formatted message, unless an exception is already
/// pending (never mask the more specific inner error).
unsafe fn set_type_error(message: String) {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return;
    }
    if let Ok(cmsg) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                cmsg.as_ptr(),
            );
        }
    }
}

/// CPython `null_error()`: SystemError for a NULL argument to an internal
/// routine.
unsafe fn set_null_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            c"null argument to internal routine".as_ptr(),
        );
    }
}

unsafe fn type_name(o: *mut PyObject) -> String {
    unsafe { crate::api::object::type_name_lossy(o) }
}

/// The object's `tp_as_sequence` slot table, if any (foreign C objects and any
/// ABI-layout type; a bridge proxy's static type usually has none).
unsafe fn seq_methods(o: *mut PyObject) -> Option<*mut PySequenceMethods> {
    if o.is_null() {
        return None;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return None;
    }
    let m = unsafe { (*tp).tp_as_sequence }.cast::<PySequenceMethods>();
    if m.is_null() { None } else { Some(m) }
}

/// Non-null `mp_subscript` presence (for CPython's "is not a sequence" vs
/// "does not support indexing" message split).
unsafe fn has_mp_subscript(o: *mut PyObject) -> bool {
    if o.is_null() {
        return false;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return false;
    }
    let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
    !m.is_null() && !unsafe { (*m).mp_subscript }.is_null()
}

/// Foreign `sq_length`, when present: calls the slot and returns its result.
unsafe fn foreign_sq_length(o: *mut PyObject) -> Option<Py_ssize_t> {
    let m = unsafe { seq_methods(o) }?;
    let f = unsafe { (*m).sq_length };
    if f.is_null() {
        return None;
    }
    let f: LenFunc = unsafe { std::mem::transmute::<*mut c_void, LenFunc>(f) };
    Some(unsafe { f(o) })
}

/// UTF-8 bytes of a native str handle (pointer valid until the next GC cycle).
unsafe fn str_slice(bits: u64) -> Option<&'static [u8]> {
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let p = unsafe { (h.str_data)(bits, &raw mut len) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(p, len) })
    }
}

/// Raw bytes of a native bytes handle.
unsafe fn bytes_slice(bits: u64) -> Option<&'static [u8]> {
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let p = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(p, len) })
    }
}

/// An identity-preserving physical snapshot. Every pointer is a new reference,
/// so self-extension, allocation, and iterator callbacks cannot invalidate an
/// element between observation and container publication.
pub(crate) struct MaterializedPointers {
    pointers: Vec<*mut PyObject>,
}

impl MaterializedPointers {
    /// Builtin mutation addresses physical list storage, including subtypes;
    /// it must never redispatch through an overridden iteration protocol.
    pub(crate) unsafe fn from_list_storage(object: *mut PyObject) -> Option<Self> {
        let read = unsafe { crate::api::sequences::ListRead::acquire(object) }?;
        let len = unsafe { read.len() };
        let mut result = Self::with_capacity(len)?;
        for index in 0..len {
            let item = unsafe { read.item(index) };
            if item.is_null() {
                return None;
            }
            unsafe { crate::api::refcount::Py_INCREF(item) };
            result.pointers.push(item);
        }
        Some(result)
    }

    fn with_capacity(capacity: usize) -> Option<Self> {
        let mut pointers = Vec::new();
        if pointers.try_reserve_exact(capacity).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        Some(Self { pointers })
    }

    pub(crate) fn len(&self) -> usize {
        self.pointers.len()
    }

    fn take(&mut self, index: usize) -> *mut PyObject {
        std::mem::replace(&mut self.pointers[index], ptr::null_mut())
    }

    pub(crate) fn as_slice(&self) -> &[*mut PyObject] {
        &self.pointers
    }
}

impl Drop for MaterializedPointers {
    fn drop(&mut self) {
        unsafe { crate::api::errors::release_preserving_error(&self.pointers) };
    }
}

fn checked_py_ssize(length: usize) -> Option<Py_ssize_t> {
    if length > Py_ssize_t::MAX as usize {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        None
    } else {
        Some(length as Py_ssize_t)
    }
}

/// Consume a materialized pointer snapshot into the canonical exact-size list
/// publisher. Every snapshot pointer is already an owned reference, so this
/// path transfers it directly without redundant reference-count traffic.
unsafe fn list_from_materialized_pointers(mut items: MaterializedPointers) -> *mut PyObject {
    let Some(size) = checked_py_ssize(items.len()) else {
        return ptr::null_mut();
    };
    unsafe {
        crate::api::sequences::list_from_owned_indexed(size, |index| items.take(index as usize))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SequenceMaterialization {
    List,
    Tuple,
    Fast,
}

/// The one C-visible iterable construction authority. Native list/tuple inputs
/// take the indexed fast path; all others use their actual iterator. Carrying
/// exact pointers deletes value-bit rematerialization and preserves `is`.
pub(crate) unsafe fn materialize_iterable_pointers(
    o: *mut PyObject,
    fast_error_message: Option<*const c_char>,
    kind: SequenceMaterialization,
) -> Option<MaterializedPointers> {
    if unsafe { crate::api::sequences::PyList_CheckExact(o) } != 0 {
        return unsafe { MaterializedPointers::from_list_storage(o) };
    }
    if unsafe { crate::api::sequences::PyTuple_CheckExact(o) } != 0 {
        let len = unsafe { crate::api::sequences::PyTuple_Size(o) };
        if len < 0 {
            return None;
        }
        let mut out = MaterializedPointers::with_capacity(len as usize)?;
        for index in 0..len {
            let pointer = unsafe { crate::api::sequences::PyTuple_GetItem(o, index) };
            if pointer.is_null() {
                return None;
            }
            unsafe { crate::api::refcount::Py_INCREF(pointer) };
            out.pointers.push(pointer);
        }
        return Some(out);
    }

    let iter = unsafe { crate::api::object::PyObject_GetIter(o) };
    if iter.is_null() {
        if let Some(message) = fast_error_message
            && unsafe {
                crate::api::errors::PyErr_ExceptionMatches(
                    (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                )
            } != 0
        {
            unsafe {
                crate::api::errors::PyErr_Clear();
                set_sequence_fast_type_error(message);
            }
        } else if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { set_type_error(format!("'{}' object is not iterable", type_name(o))) };
        }
        return None;
    }
    // Root the iterator across length-hint callbacks and failed allocation.
    let iter_owner = unsafe { crate::api::refcount::OwnedPyObject::from_owned(iter) };
    let consult = !matches!(kind, SequenceMaterialization::Tuple)
        || unsafe { (hooks_or_stubs().tuple_uses_length_hint)() };
    let hint = if consult {
        let source = if matches!(kind, SequenceMaterialization::Fast) {
            iter
        } else {
            o
        };
        let hint = unsafe { crate::api::object::PyObject_LengthHint(source, 8) };
        if hint < 0 {
            return None;
        }
        hint as usize
    } else {
        0
    };
    let mut out = MaterializedPointers::with_capacity(hint)?;
    loop {
        let item = unsafe { crate::api::object::PyIter_Next(iter) };
        if item.is_null() {
            break;
        }
        if out.pointers.try_reserve(1).is_err() {
            unsafe {
                crate::api::errors::PyErr_NoMemory();
                crate::api::errors::release_preserving_error(&[item]);
            }
            return None;
        }
        out.pointers.push(item);
    }
    drop(iter_owner);
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return None;
    }
    Some(out)
}

/// Append the code points of a native str handle to `out` as fresh 1-char str
/// handles (CPython `str` iteration yields 1-char strings).
/// Materialize any iterable into a vector of owned Molt handle bits, or None
/// with the CPython-shaped exception set.
///
/// Tiers: native list/tuple (direct index copy) → native str/bytes (element
/// semantics per CPython) → native dict (keys, via the `dict_entry` cursor) →
/// the object's own iterator protocol (`PyObject_GetIter` + `PyIter_Next`, the
/// CPython fallback for every other iterable) → TypeError.
unsafe fn set_sequence_fast_type_error(message: *const c_char) {
    let msg = if message.is_null() {
        c"object is not a sequence".as_ptr()
    } else {
        // Validate that the caller supplied a C string before handing it to
        // the shared error state.
        let _ = unsafe { CStr::from_ptr(message) };
        message
    };
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            msg,
        )
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Length(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PySequence_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Size(o: *mut PyObject) -> Py_ssize_t {
    // CPython: sq_length when present; a mapping without sq_length is "%.200s
    // is not a sequence"; anything else "object of type '%.200s' has no
    // len()". Every -1 carries an exception (sentinel sweep).
    if o.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);
        if tag == tag_list() {
            return unsafe { (h.list_len)(bits) as Py_ssize_t };
        }
        if tag == tag_tuple() {
            return unsafe { (h.tuple_len)(bits) as Py_ssize_t };
        }
        if tag == tag_str() {
            // len(str) counts CODE POINTS, not UTF-8 bytes.
            if let Some(bytes) = unsafe { str_slice(bits) }
                && let Ok(text) = std::str::from_utf8(bytes)
            {
                return text.chars().count() as Py_ssize_t;
            }
        }
        if tag == tag_bytes()
            && let Some(bytes) = unsafe { bytes_slice(bits) }
        {
            return bytes.len() as Py_ssize_t;
        }
        if tag == tag_dict() {
            // dict has mp_length but NO sq_length: CPython raises the
            // "is not a sequence" TypeError here, never returns the length.
            unsafe { set_type_error(format!("{} is not a sequence", type_name(o))) };
            return -1;
        }
    }
    // Foreign tier: dispatch the object's own sq_length.
    if let Some(n) = unsafe { foreign_sq_length(o) } {
        return n;
    }
    unsafe {
        if has_mp_subscript(o) {
            set_type_error(format!("{} is not a sequence", type_name(o)));
        } else {
            set_type_error(format!("object of type '{}' has no len()", type_name(o)));
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_GetItem(o: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let Some(resolved) = crate::bridge::observe_pyobject(o) else {
        return ptr::null_mut();
    };
    if let crate::bridge::ResolvedPyObject::ManagedMolt(value) = resolved {
        let bits = value.bits();
        let h = hooks_or_stubs();
        let tag = classify(bits);
        let semantic_type = unsafe { crate::bridge::semantic_type_for_resolved(o, resolved) };
        if semantic_type.is_null() {
            return ptr::null_mut();
        }

        if tag == tag_list() && std::ptr::eq(semantic_type, &raw mut crate::abi_types::PyList_Type) {
            let len = unsafe { (h.list_len)(bits) };
            let actual_i = if i < 0 { len as Py_ssize_t + i } else { i };
            if actual_i < 0 || actual_i >= len as Py_ssize_t {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_IndexError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"list index out of range".as_ptr(),
                    );
                }
                return ptr::null_mut();
            }
            let Some(pointer) = GLOBAL_BRIDGE.list_view_item_pointer(bits, actual_i as usize)
            else {
                unsafe { set_null_error() };
                return ptr::null_mut();
            };
            unsafe { crate::api::refcount::Py_INCREF(pointer) };
            return pointer;
        }
        if tag == tag_tuple() && std::ptr::eq(semantic_type, &raw mut crate::abi_types::PyTuple_Type) {
            let len = unsafe { (h.tuple_len)(bits) };
            let actual_i = if i < 0 { len as Py_ssize_t + i } else { i };
            if actual_i < 0 || actual_i >= len as Py_ssize_t {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_IndexError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"tuple index out of range".as_ptr(),
                    );
                }
                return ptr::null_mut();
            }
            // An exact tuple's packed C projection is the C-visible identity
            // authority. Reading the runtime value bits here and materializing
            // another carrier loses `is`, creates avoidable allocation/refcount
            // traffic, and gives the iterator path a second ownership model.
            // PyTuple_GetItem reads the physical slot; PySequence_GetItem owns
            // the one required new reference.
            let pointer =
                unsafe { crate::api::sequences::PyTuple_GetItem(o, actual_i as Py_ssize_t) };
            if pointer.is_null() {
                return ptr::null_mut();
            }
            unsafe { crate::api::refcount::Py_INCREF(pointer) };
            return pointer;
        }
        if tag == tag_str() && std::ptr::eq(semantic_type, &raw mut crate::abi_types::PyUnicode_Type) {
            // str sq_item yields a 1-code-point str (code-point indexing).
            if let Some(bytes) = unsafe { str_slice(bits) }
                && let Ok(text) = std::str::from_utf8(bytes)
            {
                let n = text.chars().count() as Py_ssize_t;
                let actual_i = if i < 0 { n + i } else { i };
                if actual_i < 0 || actual_i >= n {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_IndexError)
                                .cast::<crate::abi_types::PyObject>(),
                            c"string index out of range".as_ptr(),
                        );
                    }
                    return ptr::null_mut();
                }
                if let Some(ch) = text.chars().nth(actual_i as usize) {
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    let cb = unsafe { (h.alloc_str)(s.as_ptr(), s.len()) };
                    if cb != 0 {
                        return unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(cb) };
                    }
                }
                return ptr::null_mut();
            }
        }
        if tag == tag_bytes() && std::ptr::eq(semantic_type, &raw mut crate::abi_types::PyBytes_Type) {
            // bytes sq_item yields an int in [0, 256).
            if let Some(bytes) = unsafe { bytes_slice(bits) } {
                let n = bytes.len() as Py_ssize_t;
                let actual_i = if i < 0 { n + i } else { i };
                if actual_i < 0 || actual_i >= n {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_IndexError)
                                .cast::<crate::abi_types::PyObject>(),
                            c"index out of range".as_ptr(),
                        );
                    }
                    return ptr::null_mut();
                }
                let ib = unsafe { (h.int_from_i64)(bytes[actual_i as usize] as i64) };
                if ib != 0 {
                    return unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(ib) };
                }
                return ptr::null_mut();
            }
        }
        return unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj((h.sequence_item)(bits, i)) };
    }
    // Foreign tier: sq_item with CPython's negative-index adjustment.
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_item = unsafe { (*m).sq_item };
        if !sq_item.is_null() {
            let mut idx = i;
            if idx < 0
                && let Some(l) = unsafe { foreign_sq_length(o) }
            {
                if l < 0 {
                    return ptr::null_mut();
                }
                idx += l;
            }
            let f: SsizeArgFunc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(sq_item) };
            return unsafe { f(o, idx) };
        }
    }
    unsafe {
        if has_mp_subscript(o) {
            set_type_error(format!("{} is not a sequence", type_name(o)));
        } else {
            set_type_error(format!(
                "'{}' object does not support indexing",
                type_name(o)
            ));
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_SetItem(
    o: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    // CPython sequence_setitem: dispatch sq_ass_item with negative-index
    // adjustment; types without it (tuple, str, bytes) raise TypeError. NOTE:
    // unlike PyList_SetItem this does NOT steal the reference to v.
    if o.is_null() || v.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_list() {
            let h = hooks_or_stubs();
            let len = unsafe { (h.list_len)(bits) } as Py_ssize_t;
            let actual_i = if i < 0 { len + i } else { i };
            // PyList_SetItem steals; take an extra reference to preserve the
            // non-stealing sq_ass_item contract on success AND error paths.
            unsafe { crate::api::refcount::Py_INCREF(v) };
            return unsafe { crate::api::sequences::PyList_SetItem(o, actual_i, v) };
        }
        if tag == tag_tuple() || tag == tag_str() || tag == tag_bytes() {
            // CPython: these types have no sq_ass_item — TypeError, not the
            // previous silent tuple mutation / bare -1.
            unsafe {
                set_type_error(format!(
                    "'{}' object does not support item assignment",
                    type_name(o)
                ));
            }
            return -1;
        }
    }
    // Foreign tier: sq_ass_item(o, i, v).
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_ass = unsafe { (*m).sq_ass_item };
        if !sq_ass.is_null() {
            let mut idx = i;
            if idx < 0
                && let Some(l) = unsafe { foreign_sq_length(o) }
            {
                if l < 0 {
                    return -1;
                }
                idx += l;
            }
            let f: SsizeObjArgProc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeObjArgProc>(sq_ass) };
            return unsafe { f(o, idx, v) };
        }
    }
    unsafe {
        set_type_error(format!(
            "'{}' object does not support item assignment",
            type_name(o)
        ));
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_DelItem(o: *mut PyObject, i: Py_ssize_t) -> c_int {
    // CPython: sq_ass_item(o, i, NULL) deletes. Native list deletion routes
    // through the list_set_slice splice authority (the previous body was an
    // unconditional silent -1).
    if o.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = observed_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);
        if tag == tag_list() {
            let len = unsafe { (h.list_len)(bits) } as Py_ssize_t;
            let actual_i = if i < 0 { len + i } else { i };
            if actual_i < 0 || actual_i >= len {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_IndexError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"list assignment index out of range".as_ptr(),
                    );
                }
                return -1;
            }
            let rc = unsafe {
                crate::api::sequences::PyList_SetSlice(o, actual_i, actual_i + 1, ptr::null_mut())
            };
            if rc != 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return -1;
            }
            return 0;
        }
    }
    // Foreign tier: sq_ass_item(o, i, NULL).
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_ass = unsafe { (*m).sq_ass_item };
        if !sq_ass.is_null() {
            let mut idx = i;
            if idx < 0
                && let Some(l) = unsafe { foreign_sq_length(o) }
            {
                if l < 0 {
                    return -1;
                }
                idx += l;
            }
            let f: SsizeObjArgProc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeObjArgProc>(sq_ass) };
            return unsafe { f(o, idx, ptr::null_mut()) };
        }
    }
    unsafe {
        set_type_error(format!(
            "'{}' object doesn't support item deletion",
            type_name(o)
        ));
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Contains(o: *mut PyObject, value: *mut PyObject) -> c_int {
    if o.is_null() || value.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if GLOBAL_BRIDGE.managed_handle_for_pyobj(o).is_some() {
        // Managed class semantics belong to molt_contains, including overrides,
        // hash admission, byte substrings and arithmetic range membership.
        let Some(container) = (unsafe { crate::bridge::RuntimeValue::acquire(o) }) else {
            return -1;
        };
        let Some(needle) = (unsafe { crate::bridge::RuntimeValue::acquire(value) }) else {
            return -1;
        };
        let result = unsafe { (hooks_or_stubs().object_contains)(container.bits(), needle.bits()) };
        if unsafe { crate::api::errors::check_native_status(result, "object containment inquiry") }
            < 0
        {
            return -1;
        }
        return result;
    }
    // Native types retain CPython's physical sq_contains, then iterator search.
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_contains = unsafe { (*m).sq_contains };
        if !sq_contains.is_null() {
            let f: ObjObjProc =
                unsafe { std::mem::transmute::<*mut c_void, ObjObjProc>(sq_contains) };
            return unsafe { f(o, value) };
        }
    }
    unsafe { iter_search(o, value, IterSearch::Contains) as c_int }
}

/// CPython `_PySequence_IterSearch` operations.
enum IterSearch {
    Contains,
    Count,
    Index,
}

/// One owned iterator search for count, index and native containment fallback.
/// The iterator owns live sequence traversal; no borrowed element or sampled
/// list length survives an equality callback. Cleanup preserves the exact error.
unsafe fn iter_search(o: *mut PyObject, value: *mut PyObject, mode: IterSearch) -> Py_ssize_t {
    use crate::api::refcount::OwnedPyObject;
    let iter = unsafe { OwnedPyObject::from_owned(crate::api::object::PyObject_GetIter(o)) };
    if iter.as_ptr().is_null() {
        // Match CPython's noniterable diagnostic without replacing arbitrary
        // exceptions raised by __iter__ or its descriptor.
        if unsafe {
            crate::api::errors::PyErr_ExceptionMatches(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
            )
        } != 0
        {
            unsafe {
                set_type_error(format!(
                    "argument of type '{}' is not iterable",
                    type_name(o)
                ));
            }
        }
        return -1;
    }
    let mut position: Py_ssize_t = 0;
    let mut wrapped = false;
    loop {
        let item =
            unsafe { OwnedPyObject::from_owned(crate::api::object::PyIter_Next(iter.as_ptr())) };
        if item.as_ptr().is_null() {
            if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                return -1;
            }
            break;
        }
        let equal =
            unsafe { crate::api::typeobj::PyObject_RichCompareBool(item.as_ptr(), value, PY_EQ) };
        drop(item);
        if equal < 0 {
            return -1;
        }
        if equal != 0 {
            match mode {
                IterSearch::Contains => return 1,
                IterSearch::Count => {
                    let Some(next) = position.checked_add(1) else {
                        unsafe {
                            crate::api::errors::PyErr_SetString(
                                (&raw mut crate::abi_types::PyExc_OverflowError).cast(),
                                c"count exceeds C integer size".as_ptr(),
                            );
                        }
                        return -1;
                    };
                    position = next;
                }
                IterSearch::Index => {
                    if wrapped {
                        unsafe {
                            crate::api::errors::PyErr_SetString(
                                (&raw mut crate::abi_types::PyExc_OverflowError).cast(),
                                c"index exceeds C integer size".as_ptr(),
                            );
                        }
                        return -1;
                    }
                    return position;
                }
            }
        }
        if matches!(mode, IterSearch::Index) {
            // Overflow becomes an error only if a later item matches. A long
            // exhausted search still raises ValueError, as CPython does.
            if let Some(next) = position.checked_add(1) {
                position = next;
            } else {
                wrapped = true;
            }
        }
    }
    match mode {
        IterSearch::Contains => 0,
        IterSearch::Count => position,
        IterSearch::Index => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                    c"sequence.index(x): x not in sequence".as_ptr(),
                );
            }
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Concat(s1: *mut PyObject, s2: *mut PyObject) -> *mut PyObject {
    if s1.is_null() || s2.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let bits1 = observed_bits(s1);
    let bits2 = observed_bits(s2);
    if let Some(bits1) = bits1 {
        let h = hooks_or_stubs();
        let tag1 = classify(bits1);
        let tag2 = bits2.map(classify);

        if tag1 == tag_list() {
            // CPython list_concat: the right operand must be a list.
            if unsafe { crate::api::sequences::PyList_Check(s2) } == 0 {
                unsafe {
                    set_type_error(format!(
                        "can only concatenate list (not \"{}\") to list",
                        type_name(s2)
                    ));
                }
                return ptr::null_mut();
            }
            let Some(left) = (unsafe { crate::api::sequences::ListRead::acquire(s1) }) else {
                return ptr::null_mut();
            };
            let Some(right) = (unsafe { crate::api::sequences::ListRead::acquire(s2) }) else {
                return ptr::null_mut();
            };
            let len1 = unsafe { left.len() };
            let len2 = unsafe { right.len() };
            let Some(total) = len1.checked_add(len2) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return ptr::null_mut();
            };
            let Some(total) = checked_py_ssize(total) else {
                return ptr::null_mut();
            };
            return unsafe {
                crate::api::sequences::list_from_borrowed_indexed(total, |index| {
                    let index = index as usize;
                    if index < len1 {
                        left.item(index)
                    } else {
                        right.item(index - len1)
                    }
                })
            };
        }
        if tag1 == tag_tuple() {
            // CPython tuple_concat: the right operand must be a tuple.
            if tag2 != Some(tag_tuple()) {
                unsafe {
                    set_type_error(format!(
                        "can only concatenate tuple (not \"{}\") to tuple",
                        type_name(s2)
                    ));
                }
                return ptr::null_mut();
            }
            let bits2 = bits2.unwrap();
            let len1 = unsafe { (h.tuple_len)(bits1) };
            let len2 = unsafe { (h.tuple_len)(bits2) };
            if len1 == 0 && unsafe { crate::api::sequences::PyTuple_CheckExact(s2) } != 0 {
                unsafe { crate::api::refcount::Py_INCREF(s2) };
                return s2;
            }
            if len2 == 0 && unsafe { crate::api::sequences::PyTuple_CheckExact(s1) } != 0 {
                unsafe { crate::api::refcount::Py_INCREF(s1) };
                return s1;
            }
            let Some(total) = len1.checked_add(len2) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return ptr::null_mut();
            };
            let Some(total) = checked_py_ssize(total) else {
                return ptr::null_mut();
            };
            let new_tuple = unsafe { crate::api::sequences::PyTuple_New(total) };
            if new_tuple.is_null() {
                return ptr::null_mut();
            }
            for i in 0..len1 {
                let item = unsafe { crate::api::sequences::PyTuple_GetItem(s1, i as Py_ssize_t) };
                if item.is_null() {
                    unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
                    return ptr::null_mut();
                }
                unsafe { crate::api::refcount::Py_INCREF(item) };
                if unsafe {
                    crate::api::sequences::PyTuple_SetItem(new_tuple, i as Py_ssize_t, item)
                } != 0
                {
                    unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
                    return ptr::null_mut();
                }
            }
            for i in 0..len2 {
                let item = unsafe { crate::api::sequences::PyTuple_GetItem(s2, i as Py_ssize_t) };
                if item.is_null() {
                    unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
                    return ptr::null_mut();
                }
                unsafe { crate::api::refcount::Py_INCREF(item) };
                if unsafe {
                    crate::api::sequences::PyTuple_SetItem(
                        new_tuple,
                        (len1 + i) as Py_ssize_t,
                        item,
                    )
                } != 0
                {
                    unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
                    return ptr::null_mut();
                }
            }
            return new_tuple;
        }
        if tag1 == tag_str() {
            if tag2 == Some(tag_str())
                && let (Some(a), Some(b)) = (unsafe { str_slice(bits1) }, unsafe {
                    str_slice(bits2.unwrap())
                })
            {
                let mut joined = Vec::with_capacity(a.len() + b.len());
                joined.extend_from_slice(a);
                joined.extend_from_slice(b);
                let nb = unsafe { (h.alloc_str)(joined.as_ptr(), joined.len()) };
                if nb != 0 {
                    return unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(nb) };
                }
                return ptr::null_mut();
            }
            unsafe {
                set_type_error(format!(
                    "can only concatenate str (not \"{}\") to str",
                    type_name(s2)
                ));
            }
            return ptr::null_mut();
        }
        if tag1 == tag_bytes() {
            if tag2 == Some(tag_bytes())
                && let (Some(a), Some(b)) = (unsafe { bytes_slice(bits1) }, unsafe {
                    bytes_slice(bits2.unwrap())
                })
            {
                let mut joined = Vec::with_capacity(a.len() + b.len());
                joined.extend_from_slice(a);
                joined.extend_from_slice(b);
                let nb = unsafe { (h.alloc_bytes)(joined.as_ptr(), joined.len()) };
                if nb != 0 {
                    return unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(nb) };
                }
                return ptr::null_mut();
            }
            unsafe {
                set_type_error(format!("can't concat {} to bytes", type_name(s2)));
            }
            return ptr::null_mut();
        }
    }
    // Foreign tier: sq_concat, then the shared reflected-aware nb_add
    // authority for user classes that expose sequence identity via sq_item but
    // implement concatenation only through __add__.
    if let Some(m) = unsafe { seq_methods(s1) } {
        let sq_concat = unsafe { (*m).sq_concat };
        if !sq_concat.is_null() {
            let f: BinaryFunc =
                unsafe { std::mem::transmute::<*mut c_void, BinaryFunc>(sq_concat) };
            return unsafe { f(s1, s2) };
        }
    }
    let Some(is_sequence) = (unsafe { sequence_check(s1) }) else {
        return ptr::null_mut();
    };
    let other_is_sequence = if is_sequence {
        let Some(value) = (unsafe { sequence_check(s2) }) else {
            return ptr::null_mut();
        };
        value
    } else {
        false
    };
    if is_sequence
        && other_is_sequence
        && let Some(result) =
            unsafe { sequence_add_numeric_fallback(SequenceNumericMode::Regular, s1, s2) }
    {
        return result;
    }
    unsafe {
        set_type_error(format!("'{}' object can't be concatenated", type_name(s1)));
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Repeat(o: *mut PyObject, count: Py_ssize_t) -> *mut PyObject {
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let reps = count.max(0) as usize; // CPython clamps negative counts to 0
    if let Some(bits) = observed_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);

        if tag == tag_list() {
            let Some(read) = (unsafe { crate::api::sequences::ListRead::acquire(o) }) else {
                return ptr::null_mut();
            };
            let len = unsafe { read.len() };
            let Some(total) = len.checked_mul(reps) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return ptr::null_mut();
            };
            let Some(total) = checked_py_ssize(total) else {
                return ptr::null_mut();
            };
            let mut source_index = 0;
            return unsafe {
                crate::api::sequences::list_from_borrowed_indexed(total, |_index| {
                    let item = read.item(source_index);
                    source_index += 1;
                    if source_index == len {
                        source_index = 0;
                    }
                    item
                })
            };
        }
        if tag == tag_tuple() {
            let len = unsafe { (h.tuple_len)(bits) };
            if reps == 1 && unsafe { crate::api::sequences::PyTuple_CheckExact(o) } != 0 {
                unsafe { crate::api::refcount::Py_INCREF(o) };
                return o;
            }
            if reps == 0 || len == 0 {
                return unsafe { crate::api::sequences::PyTuple_New(0) };
            }
            let Some(total) = len.checked_mul(reps) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return ptr::null_mut();
            };
            let Some(total) = checked_py_ssize(total) else {
                return ptr::null_mut();
            };
            let new_tuple = unsafe { crate::api::sequences::PyTuple_New(total) };
            if new_tuple.is_null() {
                return ptr::null_mut();
            }
            let mut dst = 0;
            for _ in 0..reps {
                for i in 0..len {
                    let item =
                        unsafe { crate::api::sequences::PyTuple_GetItem(o, i as Py_ssize_t) };
                    if item.is_null() {
                        unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
                        return ptr::null_mut();
                    }
                    unsafe { crate::api::refcount::Py_INCREF(item) };
                    if unsafe {
                        crate::api::sequences::PyTuple_SetItem(new_tuple, dst as Py_ssize_t, item)
                    } != 0
                    {
                        unsafe { crate::api::refcount::Py_DECREF(new_tuple) };
                        return ptr::null_mut();
                    }
                    dst += 1;
                }
            }
            return new_tuple;
        }
        if tag == tag_str()
            && let Some(a) = unsafe { str_slice(bits) }
        {
            let repeated = a.repeat(reps);
            let nb = unsafe { (h.alloc_str)(repeated.as_ptr(), repeated.len()) };
            if nb != 0 {
                return unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(nb) };
            }
            return ptr::null_mut();
        }
        if tag == tag_bytes()
            && let Some(a) = unsafe { bytes_slice(bits) }
        {
            let repeated = a.repeat(reps);
            let nb = unsafe { (h.alloc_bytes)(repeated.as_ptr(), repeated.len()) };
            if nb != 0 {
                return unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(nb) };
            }
            return ptr::null_mut();
        }
    }
    // Foreign tier: sq_repeat, then the shared nb_multiply authority for user
    // sequence classes that implement only __mul__.
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_repeat = unsafe { (*m).sq_repeat };
        if !sq_repeat.is_null() {
            let f: SsizeArgFunc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(sq_repeat) };
            return unsafe { f(o, count) };
        }
    }
    let Some(is_sequence) = (unsafe { sequence_check(o) }) else {
        return ptr::null_mut();
    };
    if is_sequence
        && let Some(result) =
            unsafe { sequence_multiply_numeric_fallback(SequenceNumericMode::Regular, o, count) }
    {
        return result;
    }
    unsafe { set_type_error(format!("'{}' object can't be repeated", type_name(o))) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_List(o: *mut PyObject) -> *mut PyObject {
    // CPython builds an empty list and drains the iterator into it via
    // _PyList_Extend — ANY iterable converts; a non-iterable raises TypeError.
    // The previous body fabricated an EMPTY list for every non-list/tuple
    // (theater row).
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let Some(items) =
        (unsafe { materialize_iterable_pointers(o, None, SequenceMaterialization::List) })
    else {
        return ptr::null_mut();
    };
    unsafe { list_from_materialized_pointers(items) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyList_Extend(
    list: *mut PyObject,
    iterable: *mut PyObject,
) -> *mut PyObject {
    if list.is_null() || iterable.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let list_bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(list);
    let Some(list_bits) = list_bits else {
        unsafe { set_type_error("_PyList_Extend requires a list".to_string()) };
        return ptr::null_mut();
    };
    if !GLOBAL_BRIDGE.commit_list_view(list_bits.bits()) {
        return ptr::null_mut();
    }
    if let Some(iterable_bits) = GLOBAL_BRIDGE.molt_handle_for_pyobj(iterable)
        && unsafe { (hooks_or_stubs().classify_heap)(iterable_bits.bits()) }
            == crate::abi_types::MoltTypeTag::List as u8
        && !GLOBAL_BRIDGE.commit_list_view(iterable_bits.bits())
    {
        return ptr::null_mut();
    }
    if unsafe { crate::api::sequences::PyList_CheckExact(iterable) } != 0
        || unsafe { crate::api::sequences::PyTuple_CheckExact(iterable) } != 0
    {
        // Snapshot exact list/tuple sources, including self-extension, before
        // mutating the destination.
        let Some(items) = (unsafe {
            materialize_iterable_pointers(iterable, None, SequenceMaterialization::List)
        }) else {
            return ptr::null_mut();
        };
        for &item in &items.pointers {
            if unsafe { crate::api::sequences::PyList_Append(list, item) } != 0 {
                return ptr::null_mut();
            }
        }
    } else {
        // General iterators are consumed incrementally. If iteration or append
        // fails, the already-appended prefix remains visible exactly as in
        // CPython; eager draining changes generator observation and failures.
        let iter = unsafe { crate::api::object::PyObject_GetIter(iterable) };
        if iter.is_null() {
            return ptr::null_mut();
        }
        loop {
            let item = unsafe { crate::api::object::PyIter_Next(iter) };
            if item.is_null() {
                let failed = !unsafe { crate::api::errors::PyErr_Occurred() }.is_null();
                unsafe { crate::api::refcount::Py_DECREF(iter) };
                if failed {
                    return ptr::null_mut();
                }
                break;
            }
            let rc = unsafe { crate::api::sequences::PyList_Append(list, item) };
            unsafe { crate::api::refcount::Py_DECREF(item) };
            if rc != 0 {
                unsafe { crate::api::refcount::Py_DECREF(iter) };
                return ptr::null_mut();
            }
        }
    }
    unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Tuple(o: *mut PyObject) -> *mut PyObject {
    // CPython: PyObject_GetIter(v) drained into a tuple (tuple('abc') ==
    // ('a','b','c'), tuple(dict) == keys); non-iterable raises TypeError. The
    // previous body fabricated an EMPTY tuple for every non-list/tuple.
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if unsafe { crate::api::sequences::PyTuple_CheckExact(o) } != 0 {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if unsafe { crate::api::sequences::PyList_CheckExact(o) } != 0 {
        return unsafe { crate::api::sequences::PyList_AsTuple(o) };
    }
    let Some(mut items) =
        (unsafe { materialize_iterable_pointers(o, None, SequenceMaterialization::Tuple) })
    else {
        return ptr::null_mut();
    };
    let Some(tuple_len) = checked_py_ssize(items.len()) else {
        return ptr::null_mut();
    };
    let tuple = unsafe { crate::api::sequences::PyTuple_New(tuple_len) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    for index in 0..items.len() {
        let item = items.take(index);
        if unsafe { crate::api::sequences::PyTuple_SetItem(tuple, index as Py_ssize_t, item) } != 0
        {
            unsafe { crate::api::refcount::Py_DECREF(tuple) };
            return ptr::null_mut();
        }
    }
    tuple
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Count(o: *mut PyObject, value: *mut PyObject) -> Py_ssize_t {
    if o.is_null() || value.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    // CPython: _PySequence_IterSearch(COUNT) over any iterable.
    unsafe { iter_search(o, value, IterSearch::Count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Index(o: *mut PyObject, value: *mut PyObject) -> Py_ssize_t {
    if o.is_null() || value.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    unsafe { iter_search(o, value, IterSearch::Index) }
}

// ─── PySequence_Check ────────────────────────────────────────────────────

/// Fallible internal admission preserves runtime projection failures. Public
/// PySequence_Check keeps its Boolean result and leaves any error indicated.
pub unsafe fn sequence_check(o: *mut PyObject) -> Option<bool> {
    if o.is_null() {
        return Some(false);
    }
    if let Some(bits) = GLOBAL_BRIDGE.managed_handle_for_pyobj(o) {
        let result = unsafe { (hooks_or_stubs().sequence_check)(bits) };
        if result < 0 {
            unsafe { crate::api::errors::check_native_status(-1, "sequence slot inquiry") };
            return None;
        }
        return Some(result != 0);
    }
    let ty = unsafe { (*o).ob_type };
    if !ty.is_null()
        && unsafe {
            crate::api::typeobj::PyType_IsSubtype(ty, &raw mut crate::abi_types::PyDict_Type)
        } != 0
    {
        return Some(false);
    }
    Some(match unsafe { seq_methods(o) } {
        Some(methods) => !unsafe { (*methods).sq_item }.is_null(),
        None => false,
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Check(o: *mut PyObject) -> c_int {
    i32::from(unsafe { sequence_check(o) }.unwrap_or(false))
}

// ─── PySequence_Fast — fast access to list/tuple items ───────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast(
    o: *mut PyObject,
    msg: *const std::os::raw::c_char,
) -> *mut PyObject {
    // CPython accepts ANY iterable (list/tuple fast path, else the iterator
    // protocol). CPython materializes a general iterable into an exact list;
    // both exact lists and tuples already expose stable physical pointer arrays.
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if unsafe { crate::api::sequences::PyList_CheckExact(o) } != 0
        || unsafe { crate::api::sequences::PyTuple_CheckExact(o) } != 0
    {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    let Some(items) =
        (unsafe { materialize_iterable_pointers(o, Some(msg), SequenceMaterialization::Fast) })
    else {
        return ptr::null_mut();
    };
    unsafe { list_from_materialized_pointers(items) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_GET_SIZE(o: *mut PyObject) -> Py_ssize_t {
    if unsafe { crate::api::sequences::PyTuple_Check(o) } != 0 {
        return unsafe { crate::api::sequences::PyTuple_Size(o) };
    }
    unsafe { PySequence_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_GET_ITEM(
    o: *mut PyObject,
    i: Py_ssize_t,
) -> *mut PyObject {
    if unsafe { crate::api::sequences::PyTuple_Check(o) } != 0 {
        return unsafe { crate::api::sequences::PyTuple_GET_ITEM(o, i) };
    }
    unsafe { crate::api::sequences::PyList_GET_ITEM(o, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_ITEMS(o: *mut PyObject) -> *mut *mut PyObject {
    if unsafe { crate::api::sequences::PyList_CheckExact(o) } != 0 {
        if let Some(bits) = resolve_bits(o)
            && !GLOBAL_BRIDGE.commit_list_view(bits)
        {
            return ptr::null_mut();
        }
        return unsafe { (*o.cast::<PyListObject>()).ob_item };
    }
    if unsafe { crate::api::sequences::PyTuple_CheckExact(o) } != 0 {
        let tuple = o.cast::<PyTupleObject>();
        return unsafe { crate::api::sequences::tuple_items_ptr(tuple) };
    }
    ptr::null_mut()
}

// ─── PySequence_InPlaceConcat / InPlaceRepeat ────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_InPlaceConcat(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    // CPython prefers sq_inplace_concat: `list += seq` EXTENDS IN PLACE and
    // returns a new reference to the SAME object. The previous delegation to
    // Concat allocated a fresh list, so aliases never observed the extension.
    if o1.is_null() || o2.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits1) = resolve_bits(o1)
        && classify(bits1) == tag_list()
    {
        let result = unsafe { _PyList_Extend(o1, o2) };
        if result.is_null() {
            return ptr::null_mut();
        }
        unsafe {
            crate::api::refcount::Py_DECREF(result);
            crate::api::refcount::Py_INCREF(o1);
        }
        return o1;
    }
    // Foreign tier: sq_inplace_concat, then sq_concat, then the shared
    // nb_inplace_add -> reflected-aware nb_add authority.
    if let Some(m) = unsafe { seq_methods(o1) } {
        let slot = unsafe { (*m).sq_inplace_concat };
        if !slot.is_null() {
            let f: BinaryFunc = unsafe { std::mem::transmute::<*mut c_void, BinaryFunc>(slot) };
            return unsafe { f(o1, o2) };
        }
        let slot = unsafe { (*m).sq_concat };
        if !slot.is_null() {
            let f: BinaryFunc = unsafe { std::mem::transmute::<*mut c_void, BinaryFunc>(slot) };
            return unsafe { f(o1, o2) };
        }
    }
    let Some(is_sequence) = (unsafe { sequence_check(o1) }) else {
        return ptr::null_mut();
    };
    let other_is_sequence = if is_sequence {
        let Some(value) = (unsafe { sequence_check(o2) }) else {
            return ptr::null_mut();
        };
        value
    } else {
        false
    };
    if is_sequence
        && other_is_sequence
        && let Some(result) =
            unsafe { sequence_add_numeric_fallback(SequenceNumericMode::InPlace, o1, o2) }
    {
        return result;
    }
    unsafe { set_type_error(format!("'{}' object can't be concatenated", type_name(o1))) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_InPlaceRepeat(
    o: *mut PyObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    // CPython prefers sq_inplace_repeat: `list *= n` mutates in place and
    // returns a new reference to the SAME object.
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits) = resolve_bits(o)
        && classify(bits) == tag_list()
    {
        let h = hooks_or_stubs();
        let len = unsafe { (h.list_len)(bits) };
        if count <= 0 {
            // `lst *= 0` empties in place.
            let rc = unsafe {
                crate::api::sequences::PyList_SetSlice(o, 0, len as Py_ssize_t, ptr::null_mut())
            };
            if rc != 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return ptr::null_mut();
            }
        } else if count > 1 {
            let Some(snapshot) = (unsafe { MaterializedPointers::from_list_storage(o) }) else {
                return ptr::null_mut();
            };
            for _ in 1..count {
                for &item in &snapshot.pointers {
                    if unsafe { crate::api::sequences::PyList_Append(o, item) } != 0 {
                        return ptr::null_mut();
                    }
                }
            }
        }
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if let Some(m) = unsafe { seq_methods(o) } {
        let slot = unsafe { (*m).sq_inplace_repeat };
        if !slot.is_null() {
            let f: SsizeArgFunc = unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(slot) };
            return unsafe { f(o, count) };
        }
        let slot = unsafe { (*m).sq_repeat };
        if !slot.is_null() {
            let f: SsizeArgFunc = unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(slot) };
            return unsafe { f(o, count) };
        }
    }
    let Some(is_sequence) = (unsafe { sequence_check(o) }) else {
        return ptr::null_mut();
    };
    if is_sequence
        && let Some(result) =
            unsafe { sequence_multiply_numeric_fallback(SequenceNumericMode::InPlace, o, count) }
    {
        return result;
    }
    unsafe { set_type_error(format!("'{}' object can't be repeated", type_name(o))) };
    ptr::null_mut()
}

/// Index-bound slice construction owns both temporary integers. The public
/// header and linked extensions enter the same physical slice/item authority.
unsafe fn slice_from_indices(
    low: Py_ssize_t,
    high: Py_ssize_t,
) -> crate::api::refcount::OwnedPyObject {
    use crate::api::refcount::OwnedPyObject;
    unsafe {
        let start = OwnedPyObject::from_owned(crate::api::numbers::PyLong_FromSsize_t(low));
        if start.as_ptr().is_null() {
            return start;
        }
        let stop = OwnedPyObject::from_owned(crate::api::numbers::PyLong_FromSsize_t(high));
        if stop.as_ptr().is_null() {
            return stop;
        }
        OwnedPyObject::from_owned(crate::api::slice::PySlice_New(
            start.as_ptr(),
            stop.as_ptr(),
            ptr::null_mut(),
        ))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_GetSlice(
    obj: *mut PyObject,
    low: Py_ssize_t,
    high: Py_ssize_t,
) -> *mut PyObject {
    if obj.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let slice = unsafe { slice_from_indices(low, high) };
    if slice.as_ptr().is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::object::PyObject_GetItem(obj, slice.as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_SetSlice(
    obj: *mut PyObject,
    low: Py_ssize_t,
    high: Py_ssize_t,
    value: *mut PyObject,
) -> c_int {
    if obj.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    let slice = unsafe { slice_from_indices(low, high) };
    if slice.as_ptr().is_null() {
        return -1;
    }
    unsafe {
        if value.is_null() {
            crate::api::object::PyObject_DelItem(obj, slice.as_ptr())
        } else {
            crate::api::object::PyObject_SetItem(obj, slice.as_ptr(), value)
        }
    }
}
