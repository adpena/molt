//! Object bridge: bidirectional translation between `*mut PyObject` and `MoltHandle`.
//!
//! ## Design
//!
//! Every time Molt passes an argument to a C extension, or a C extension
//! returns a value to Molt, we need to translate:
//!
//! - `MoltHandle` → `*mut PyObject`: allocate a `PyObject` header on a bridge
//!   arena, fill `ob_type` from the static type registry, cache the mapping.
//!
//! - `*mut PyObject` → `MoltHandle`: look up the reverse mapping in the
//!   bridge's pointer table.
//!
//! ## SIMD-accelerated type-tag lookup
//!
//! When translating handles to PyObject pointers, we need to find the
//! corresponding `PyTypeObject*` for the Molt type tag embedded in the handle.
//! The tag table has at most 16 entries (see `MoltTypeTag`), fitting in one
//! SIMD register.
//!
//! - **x86_64 + SSE4.1**: `_mm_cmpeq_epi8` on a 16-byte tag→index table.
//! - **aarch64 + NEON**: `vceqq_u8` equivalent.
//! - **Scalar fallback**: linear scan of a 16-entry array.
//!
//! The SIMD paths reduce branch mispredictions on the argument dispatch loop
//! in `PyArg_ParseTuple`, which is called on every C extension function entry.

use crate::abi_types::{
    MoltManaged_Type, MoltTypeTag, Py_False, Py_None, Py_True, PyAttributeErrorObject,
    PyBaseExceptionGroupObject, PyBaseExceptionObject, PyBaseObject_Type, PyBool_Type,
    PyCMethodObject, PyImportErrorObject, PyList_Type, PyNameErrorObject, PyOSErrorObject,
    PyObject, PyStopIterationObject, PySyntaxErrorObject, PySystemExitObject, PyTuple_Type,
    PyType_Type, PyTypeObject, PyUnicodeErrorObject,
};
use molt_lang_obj_model::{ExceptionLayoutKind, MAX_EXCEPTION_TYPED_FIELDS, MoltObject};
use once_cell::sync::OnceCell;
use parking_lot::{Condvar, Mutex, MutexGuard};
use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ptr::NonNull;
use std::sync::Once;

/// A MoltHandle cast to u64, used as bridge map key.
pub type AbiHandle = u64;

mod slice;
mod unicode;
use unicode::UnicodeProjection;

#[cfg(test)]
mod publication_tests;

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct BridgeIdentity(AbiHandle);

impl BridgeIdentity {
    #[inline]
    pub const fn as_handle(self) -> AbiHandle {
        self.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct MoltValueHandle(AbiHandle);

impl MoltValueHandle {
    #[inline]
    pub const fn bits(self) -> AbiHandle {
        self.0
    }

    #[inline]
    pub(crate) fn decode(self) -> MoltObject {
        MoltObject::from_bits(self.0)
    }
}

/// Resolve canonical managed/scalar ABI views without interpreting arbitrary
/// pointer address bits as object values. Everything else is foreign.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum ResolvedPyObject {
    ManagedMolt(MoltValueHandle),
    Foreign,
}

#[inline]
pub(crate) fn resolve_pyobject(ptr: *mut PyObject) -> Option<ResolvedPyObject> {
    if ptr.is_null() {
        return None;
    }
    Some(match GLOBAL_BRIDGE.molt_handle_for_pyobj(ptr) {
        Some(handle) => ResolvedPyObject::ManagedMolt(handle),
        None => {
            admit_foreign_pyobject(ptr)?;
            ResolvedPyObject::Foreign
        }
    })
}

/// Membership is queried only after managed identity misses, without holding
/// any bridge lock. The runtime releases its registry lock before we allocate
/// an error; source-buffer provenance cannot license a CPython layout cast.
fn admit_foreign_pyobject(ptr: *mut PyObject) -> Option<()> {
    if unsafe { (crate::hooks::hooks_or_stubs().private_c_heap_contains)(ptr.addr()) } == 0 {
        return Some(());
    }
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
        && !crate::api::errors::transfer_runtime_pending_to_current()
    {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                c"registered private C-heap storage has no CPython object protocol".as_ptr(),
            )
        };
    }
    None
}

#[inline]
pub(crate) fn resolved_molt_handle(ptr: *mut PyObject) -> Option<MoltValueHandle> {
    GLOBAL_BRIDGE.observed_handle_for_pyobj(ptr)
}

/// Observe a numeric/protocol operand without treating a failed managed-view
/// commit as foreign identity. The caller keeps the C object alive throughout
/// the observation and reports NULL arguments according to its API contract.
pub(crate) fn observe_pyobject(ptr: *mut PyObject) -> Option<ResolvedPyObject> {
    match resolve_pyobject(ptr)? {
        ResolvedPyObject::ManagedMolt(value) => GLOBAL_BRIDGE
            .prepare_runtime_value(value, RuntimeValueAccess::Observe)
            .map(ResolvedPyObject::ManagedMolt),
        ResolvedPyObject::Foreign => Some(ResolvedPyObject::Foreign),
    }
}

/// Mapping from MoltHandle bits → allocated PyObject header.
/// Entries live until the extension signals dealloc via Py_DECREF → 0.
///
/// Identity is recovered only from the bridge maps. No object-address or
/// adjacent-memory encoding participates in the ABI contract.
#[repr(C)]
struct BridgeHeader {
    /// The CPython-layout `PyObject` header. C extensions and the bridge itself
    /// hold *aliasing* `*mut PyObject` pointers into this field and mutate
    /// `ob_refcnt` through them (that is what CPython refcounting is). Interior
    /// mutability (`UnsafeCell`) is therefore mandatory: without it, every fresh
    /// `&`/`&mut` reborrow of this field would pop previously-handed-out raw
    /// pointers off the aliasing model's borrow stack, making a later access
    /// through an earlier pointer undefined behaviour (a real miscompilation
    /// hazard — LLVM may cache/reorder around the reborrow). `UnsafeCell` is
    /// `#[repr(transparent)]`, preserving the C-visible `PyObject` prefix.
    py_obj: UnsafeCell<PyObject>,
}

/// One CPython-layout list sidecar. Runtime list storage remains canonical;
/// this separately allocated pointer array is the physical ABI view required
/// by Cython's unavoidable `((PyListObject *)list)->ob_item` construction path.
/// `shadow` distinguishes direct stealing writes from the last published
/// runtime snapshot, while `initialized` preserves PyList_New's NULL-slot
/// contract even though the runtime temporarily holds None placeholders.
struct ListAllocation {
    object: Box<UnsafeCell<crate::abi_types::PyListObject>>,
    items: Vec<*mut PyObject>,
    shadow: Vec<*mut PyObject>,
    initialized: Vec<bool>,
    uninitialized_count: usize,
    sealed: bool,
}

impl ListAllocation {
    fn new(ob_refcnt: isize, ob_type: *mut PyTypeObject, len: usize) -> Option<Self> {
        if len > crate::abi_types::Py_ssize_t::MAX as usize {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError).cast::<PyObject>(),
                    c"list is too large for Py_ssize_t".as_ptr(),
                )
            };
            return None;
        }
        let mut items: Vec<*mut PyObject> = Vec::new();
        let mut shadow: Vec<*mut PyObject> = Vec::new();
        let mut initialized: Vec<bool> = Vec::new();
        if items.try_reserve_exact(len).is_err()
            || shadow.try_reserve_exact(len).is_err()
            || initialized.try_reserve_exact(len).is_err()
        {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        items.resize(len, std::ptr::null_mut());
        shadow.resize(len, std::ptr::null_mut());
        initialized.resize(len, true);
        let ob_item = if items.is_empty() {
            std::ptr::null_mut()
        } else {
            items.as_mut_ptr()
        };
        let object = Box::new(UnsafeCell::new(crate::abi_types::PyListObject {
            ob_base: crate::abi_types::PyVarObject {
                ob_base: PyObject { ob_refcnt, ob_type },
                ob_size: len as crate::abi_types::Py_ssize_t,
            },
            ob_item,
            allocated: items.capacity() as crate::abi_types::Py_ssize_t,
        }));
        Some(Self {
            object,
            items,
            shadow,
            initialized,
            uninitialized_count: 0,
            sealed: true,
        })
    }

    #[inline]
    fn py_obj(&self) -> *mut PyObject {
        self.object.get().cast::<PyObject>()
    }

    #[inline]
    fn publish_storage(&mut self) {
        let object = unsafe { &mut *self.object.get() };
        object.ob_base.ob_size = self.items.len() as crate::abi_types::Py_ssize_t;
        object.ob_item = if self.items.is_empty() {
            std::ptr::null_mut()
        } else {
            self.items.as_mut_ptr()
        };
        object.allocated = self.items.capacity() as crate::abi_types::Py_ssize_t;
    }

    fn mark_uninitialized(&mut self) {
        self.items.fill(std::ptr::null_mut());
        self.shadow.fill(std::ptr::null_mut());
        self.initialized.fill(false);
        self.uninitialized_count = self.initialized.len();
        self.sealed = self.uninitialized_count == 0;
        self.publish_storage();
    }
}

unsafe impl Send for ListAllocation {}

/// A fully staged physical list projection. All allocation and C-reference
/// acquisition happens before the runtime mutates canonical storage. Publishing
/// is therefore allocation-free and can be ordered immediately after the
/// runtime swap but before any displaced edge is released.
pub struct PreparedListProjection {
    bits: AbiHandle,
    items: Option<Vec<*mut PyObject>>,
    shadow: Option<Vec<*mut PyObject>>,
    initialized: Option<Vec<bool>>,
}

pub struct RetiredListProjection {
    pointers: Vec<*mut PyObject>,
}

/// Both allocation buffers moved out of a canonical list projection after its
/// C-visible storage has published empty. Keeping the original buffers avoids
/// any allocation after publication, including for dirty direct-C slots.
pub struct RetiredClearedListProjection {
    items: Vec<*mut PyObject>,
    shadow: Vec<*mut PyObject>,
}

/// The mutable Type mirrors corresponding to runtime class cycle-clear slots.
/// Identity stays published: native and other C owners retire by ordinary RC.
pub struct RetiredTypeCycleProjection {
    pointers: [*mut PyObject; 2],
}

impl Drop for RetiredTypeCycleProjection {
    fn drop(&mut self) {
        crate::api::errors::with_preserved_error(|| {
            for pointer in self.pointers {
                unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
            }
        });
    }
}

/// Ordinary C-owned fields removed from a fixed physical projection. Exception
/// fields and CFunction.m_module share this deferred retirement authority;
/// private slot mirrors and list shadow ownership use their own protocols.
/// Publication is complete before construction, and no bridge lock may remain
/// held when the guard releases references and invokes arbitrary finalizers.
pub struct RetiredOwnedCFields {
    pointers: [*mut PyObject; EXCEPTION_VIEW_POINTER_FIELDS],
}

impl RetiredOwnedCFields {
    fn one(pointer: *mut PyObject) -> Self {
        let mut pointers = [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS];
        pointers[0] = pointer;
        Self { pointers }
    }
}

impl From<ExceptionViewState> for RetiredOwnedCFields {
    fn from(state: ExceptionViewState) -> Self {
        Self {
            pointers: state.pointers(),
        }
    }
}

/// One pre-acquired C projection edge for an allocation-free list delta.
/// Preparation may reserve physical pointer-array capacity, but it does not
/// change logical length or item ownership. Publication transfers `pointer`
/// into the live projection.
pub struct PreparedListValue {
    bits: AbiHandle,
    expected_len: usize,
    pointer: Option<*mut PyObject>,
}

/// A displaced physical list edge. Dropping it after runtime and projection
/// publication preserves reentrant-finalizer ordering without allocating a
/// one-element retirement vector.
pub struct RetiredListItem {
    pointer: Option<*mut PyObject>,
}

/// A stolen C reference prepared for one exact tuple slot. Tuple construction
/// is the only mutation lane: preparation adopts the caller-owned reference
/// before the runtime edge is published, and publication itself cannot
/// allocate or fail after validating the fixed slot.
pub struct PreparedTupleValue {
    bits: AbiHandle,
    index: usize,
    pointer: Option<*mut PyObject>,
}

/// A displaced exact tuple projection edge. It is released only after both the
/// runtime slot and the physical slot publish their replacements.
pub struct RetiredTupleItem {
    pointer: Option<*mut PyObject>,
}

impl Drop for RetiredListProjection {
    fn drop(&mut self) {
        for pointer in self.pointers.drain(..).filter(|pointer| !pointer.is_null()) {
            unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
        }
    }
}

impl Drop for RetiredClearedListProjection {
    fn drop(&mut self) {
        debug_assert_eq!(self.items.len(), self.shadow.len());
        for (current, projected) in self.items.drain(..).zip(self.shadow.drain(..)) {
            if !projected.is_null() {
                unsafe { GLOBAL_BRIDGE.projection_decref(projected) };
            }
            if current != projected && !current.is_null() {
                unsafe { crate::api::refcount::Py_DECREF(current) };
            }
        }
    }
}

impl Drop for RetiredOwnedCFields {
    fn drop(&mut self) {
        let pointers = std::mem::replace(
            &mut self.pointers,
            [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS],
        );
        unsafe { crate::api::errors::release_preserving_error(&pointers) };
    }
}

impl Drop for PreparedListValue {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take()
            && !pointer.is_null()
        {
            unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
        }
    }
}

impl Drop for RetiredListItem {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take()
            && !pointer.is_null()
        {
            unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
        }
    }
}

impl Drop for PreparedTupleValue {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take()
            && !pointer.is_null()
        {
            unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
        }
    }
}

impl Drop for RetiredTupleItem {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take()
            && !pointer.is_null()
        {
            unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
        }
    }
}

struct DirectListCommitCell {
    index: usize,
    pointer: *mut PyObject,
    old_projection: *mut PyObject,
    new_bits: AbiHandle,
    old_bits: Option<AbiHandle>,
    rollback_displaced: Option<AbiHandle>,
}

impl PreparedListProjection {
    /// # Safety
    /// The caller must have atomically installed the matching runtime handle
    /// sequence while holding the runtime GIL/list mutation authority.
    pub unsafe fn publish(mut self) -> RetiredListProjection {
        GLOBAL_BRIDGE.publish_prepared_list_projection(&mut self)
    }

    /// Reorder an already-owned complete projection without changing any C
    /// reference count. `order[dst]` names the original source slot.
    pub fn reorder<I>(&mut self, order: I) -> bool
    where
        I: IntoIterator<Item = usize>,
    {
        let Some(items) = self.items.as_mut() else {
            return false;
        };
        let Some(shadow) = self.shadow.as_mut() else {
            return false;
        };
        if shadow.len() != items.len() {
            return false;
        }
        let mut written = 0usize;
        for (dst, src) in order.into_iter().enumerate() {
            if dst >= items.len() {
                return false;
            }
            let Some(pointer) = items.get(src).copied() else {
                return false;
            };
            shadow[dst] = pointer;
            written += 1;
        }
        if written != items.len() {
            return false;
        }
        std::mem::swap(items, shadow);
        shadow.copy_from_slice(items);
        if let Some(initialized) = self.initialized.as_mut() {
            initialized.fill(true);
        }
        true
    }
}

impl PreparedListValue {
    /// Publish a pre-reserved insertion after the matching runtime vector has
    /// changed. This performs no allocation and transfers the staged C edge.
    pub unsafe fn publish_insert(mut self, index: usize) -> bool {
        let Some(pointer) = self.pointer else {
            return false;
        };
        let published = {
            let mut handle = GLOBAL_BRIDGE.handle_shard(self.bits).lock();
            let Some(entry) = handle.to_py.get_mut(&self.bits) else {
                return false;
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                return false;
            };
            if !allocation.sealed
                || allocation.items != allocation.shadow
                || allocation.items.len() != self.expected_len
                || index > self.expected_len
                || allocation.items.capacity() <= self.expected_len
                || allocation.shadow.capacity() <= self.expected_len
                || allocation.initialized.capacity() <= self.expected_len
            {
                return false;
            }
            allocation.items.insert(index, pointer);
            allocation.shadow.insert(index, pointer);
            allocation.initialized.insert(index, true);
            allocation.publish_storage();
            true
        };
        if published {
            self.pointer = None;
        }
        published
    }

    /// Publish a pre-acquired indexed replacement and return the displaced
    /// physical edge for post-publication release.
    pub unsafe fn publish_set(mut self, index: usize) -> Option<RetiredListItem> {
        let pointer = self.pointer?;
        let old = {
            let mut handle = GLOBAL_BRIDGE.handle_shard(self.bits).lock();
            let entry = handle.to_py.get_mut(&self.bits)?;
            let ManagedView::List { allocation } = &mut entry.view else {
                return None;
            };
            if !allocation.sealed
                || allocation.items != allocation.shadow
                || allocation.items.len() != self.expected_len
                || index >= self.expected_len
            {
                return None;
            }
            let old = allocation.shadow[index];
            allocation.items[index] = pointer;
            allocation.shadow[index] = pointer;
            old
        };
        self.pointer = None;
        Some(RetiredListItem { pointer: Some(old) })
    }
}

impl PreparedTupleValue {
    /// Publish the already-adopted C edge into the canonical packed tuple
    /// projection. The runtime fixed-slot write must have committed first.
    pub unsafe fn publish(mut self) -> Option<RetiredTupleItem> {
        let pointer = self.pointer?;
        let old = {
            let mut handle = GLOBAL_BRIDGE.handle_shard(self.bits).lock();
            let entry = handle.to_py.get_mut(&self.bits)?;
            let ManagedView::Tuple { allocation } = &mut entry.view else {
                return None;
            };
            let slot = allocation.items_mut().get_mut(self.index)?;
            std::mem::replace(slot, pointer)
        };
        self.pointer = None;
        Some(RetiredTupleItem { pointer: Some(old) })
    }
}

impl Drop for PreparedListProjection {
    fn drop(&mut self) {
        let Some(items) = self.items.take() else {
            return;
        };
        for pointer in items.into_iter().filter(|pointer| !pointer.is_null()) {
            unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
        }
    }
}

/// One CPython-layout variable-sized tuple allocation.  `PyTupleObject` owns
/// its item vector inline; keeping a second `Box<[*mut PyObject]>` and storing
/// its address in `ob_item` describes a different object representation and
/// breaks prebuilt `PyTuple_GET_ITEM` code.
struct TupleAllocation {
    object: NonNull<crate::abi_types::PyTupleObject>,
    layout: std::alloc::Layout,
    len: usize,
}

impl TupleAllocation {
    fn new(ob_refcnt: isize, ob_type: *mut PyTypeObject, len: usize) -> Option<Self> {
        if len > crate::abi_types::Py_ssize_t::MAX as usize {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError).cast::<PyObject>(),
                    c"tuple is too large for Py_ssize_t".as_ptr(),
                )
            };
            return None;
        }
        let item_offset = std::mem::offset_of!(crate::abi_types::PyTupleObject, ob_item);
        let Some(item_bytes) = len.checked_mul(std::mem::size_of::<*mut PyObject>()) else {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError).cast::<PyObject>(),
                    c"tuple allocation size overflow".as_ptr(),
                )
            };
            return None;
        };
        let Some(required) = item_offset.checked_add(item_bytes) else {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError).cast::<PyObject>(),
                    c"tuple allocation size overflow".as_ptr(),
                )
            };
            return None;
        };
        // A zero-length tuple has no readable item, but retaining the complete
        // declared object size keeps Rust initialization in-bounds.
        let size = required.max(std::mem::size_of::<crate::abi_types::PyTupleObject>());
        let Ok(layout) = std::alloc::Layout::from_size_align(
            size,
            std::mem::align_of::<crate::abi_types::PyTupleObject>(),
        ) else {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError).cast::<PyObject>(),
                    c"tuple allocation layout overflow".as_ptr(),
                )
            };
            return None;
        };
        let allocation = unsafe { std::alloc::alloc_zeroed(layout) };
        let Some(object) = NonNull::new(allocation.cast::<crate::abi_types::PyTupleObject>())
        else {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        };
        unsafe {
            object.as_ptr().write(crate::abi_types::PyTupleObject {
                ob_base: crate::abi_types::PyVarObject {
                    ob_base: PyObject { ob_refcnt, ob_type },
                    ob_size: len as crate::abi_types::Py_ssize_t,
                },
                ob_item: [std::ptr::null_mut()],
            });
        }
        Some(Self {
            object,
            layout,
            len,
        })
    }

    #[inline]
    fn py_obj(&self) -> *mut PyObject {
        self.object.as_ptr().cast::<PyObject>()
    }

    #[inline]
    fn items_ptr(&self) -> *mut *mut PyObject {
        unsafe { std::ptr::addr_of_mut!((*self.object.as_ptr()).ob_item).cast() }
    }

    fn items(&self) -> &[*mut PyObject] {
        unsafe { std::slice::from_raw_parts(self.items_ptr(), self.len) }
    }

    fn items_mut(&mut self) -> &mut [*mut PyObject] {
        unsafe { std::slice::from_raw_parts_mut(self.items_ptr(), self.len) }
    }
}

impl Drop for TupleAllocation {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.object.as_ptr().cast::<u8>(), self.layout) };
    }
}

unsafe impl Send for TupleAllocation {}

const EXCEPTION_BASE_POINTER_FIELDS: usize = 6;
pub const EXCEPTION_VIEW_POINTER_FIELDS: usize =
    EXCEPTION_BASE_POINTER_FIELDS + MAX_EXCEPTION_TYPED_FIELDS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExceptionViewState {
    layout_kind: ExceptionLayoutKind,
    base: [*mut PyObject; EXCEPTION_BASE_POINTER_FIELDS],
    typed: [*mut PyObject; MAX_EXCEPTION_TYPED_FIELDS],
    suppress_context: std::os::raw::c_char,
    unicode_start: isize,
    unicode_end: isize,
    os_error_written: isize,
}

impl ExceptionViewState {
    fn empty(layout_kind: ExceptionLayoutKind) -> Self {
        Self {
            layout_kind,
            base: [std::ptr::null_mut(); EXCEPTION_BASE_POINTER_FIELDS],
            typed: [std::ptr::null_mut(); MAX_EXCEPTION_TYPED_FIELDS],
            suppress_context: 0,
            unicode_start: 0,
            unicode_end: 0,
            os_error_written: -1,
        }
    }

    fn pointers(self) -> [*mut PyObject; EXCEPTION_VIEW_POINTER_FIELDS] {
        let mut pointers = [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS];
        pointers[..EXCEPTION_BASE_POINTER_FIELDS].copy_from_slice(&self.base);
        pointers[EXCEPTION_BASE_POINTER_FIELDS..].copy_from_slice(&self.typed);
        pointers
    }
}

/// Owns one exactly sized CPython 3.12 exception object. The enum is the sole
/// allocation authority; no typed exception may fall back to a base-sized
/// sidecar while advertising a larger `tp_basicsize`.
enum ExceptionAllocation {
    Base(Box<UnsafeCell<PyBaseExceptionObject>>),
    Group(Box<UnsafeCell<PyBaseExceptionGroupObject>>),
    Syntax(Box<UnsafeCell<PySyntaxErrorObject>>),
    Import(Box<UnsafeCell<PyImportErrorObject>>),
    Unicode(Box<UnsafeCell<PyUnicodeErrorObject>>),
    SystemExit(Box<UnsafeCell<PySystemExitObject>>),
    OSError(Box<UnsafeCell<PyOSErrorObject>>),
    StopIteration(Box<UnsafeCell<PyStopIterationObject>>),
    NameError(Box<UnsafeCell<PyNameErrorObject>>),
    AttributeError(Box<UnsafeCell<PyAttributeErrorObject>>),
}

unsafe impl Send for ExceptionAllocation {}

impl ExceptionAllocation {
    fn new(layout_kind: ExceptionLayoutKind, ob_refcnt: isize, ob_type: *mut PyTypeObject) -> Self {
        let mut allocation = match layout_kind {
            ExceptionLayoutKind::Base => {
                Self::Base(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::Group => {
                Self::Group(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::Syntax => {
                Self::Syntax(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::Import => {
                Self::Import(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::Unicode => {
                Self::Unicode(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::SystemExit => {
                Self::SystemExit(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::OSError => {
                Self::OSError(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::StopIteration => {
                Self::StopIteration(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::NameError => {
                Self::NameError(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
            ExceptionLayoutKind::AttributeError => {
                Self::AttributeError(Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })))
            }
        };
        unsafe {
            allocation.base_mut().ob_base = PyObject { ob_refcnt, ob_type };
            if let Self::OSError(object) = &mut allocation {
                (*object.get()).written = -1;
            }
        }
        allocation
    }

    fn layout_kind(&self) -> ExceptionLayoutKind {
        match self {
            Self::Base(_) => ExceptionLayoutKind::Base,
            Self::Group(_) => ExceptionLayoutKind::Group,
            Self::Syntax(_) => ExceptionLayoutKind::Syntax,
            Self::Import(_) => ExceptionLayoutKind::Import,
            Self::Unicode(_) => ExceptionLayoutKind::Unicode,
            Self::SystemExit(_) => ExceptionLayoutKind::SystemExit,
            Self::OSError(_) => ExceptionLayoutKind::OSError,
            Self::StopIteration(_) => ExceptionLayoutKind::StopIteration,
            Self::NameError(_) => ExceptionLayoutKind::NameError,
            Self::AttributeError(_) => ExceptionLayoutKind::AttributeError,
        }
    }

    unsafe fn base(&self) -> &PyBaseExceptionObject {
        unsafe {
            match self {
                Self::Base(object) => &*object.get(),
                Self::Group(object) => &(*object.get()).base,
                Self::Syntax(object) => &(*object.get()).base,
                Self::Import(object) => &(*object.get()).base,
                Self::Unicode(object) => &(*object.get()).base,
                Self::SystemExit(object) => &(*object.get()).base,
                Self::OSError(object) => &(*object.get()).base,
                Self::StopIteration(object) => &(*object.get()).base,
                Self::NameError(object) => &(*object.get()).base,
                Self::AttributeError(object) => &(*object.get()).base,
            }
        }
    }

    unsafe fn base_mut(&mut self) -> &mut PyBaseExceptionObject {
        unsafe {
            match self {
                Self::Base(object) => &mut *object.get(),
                Self::Group(object) => &mut (*object.get()).base,
                Self::Syntax(object) => &mut (*object.get()).base,
                Self::Import(object) => &mut (*object.get()).base,
                Self::Unicode(object) => &mut (*object.get()).base,
                Self::SystemExit(object) => &mut (*object.get()).base,
                Self::OSError(object) => &mut (*object.get()).base,
                Self::StopIteration(object) => &mut (*object.get()).base,
                Self::NameError(object) => &mut (*object.get()).base,
                Self::AttributeError(object) => &mut (*object.get()).base,
            }
        }
    }

    fn py_obj(&self) -> *mut PyObject {
        unsafe {
            std::ptr::from_ref(self.base())
                .cast_mut()
                .cast::<PyObject>()
        }
    }

    unsafe fn state(&self) -> ExceptionViewState {
        let mut state = ExceptionViewState::empty(self.layout_kind());
        unsafe {
            let base = self.base();
            state.base = [
                base.dict,
                base.args,
                base.notes,
                base.traceback,
                base.context,
                base.cause,
            ];
            state.suppress_context = base.suppress_context;
            match self {
                Self::Base(_) => {}
                Self::Group(object) => {
                    let object = &*object.get();
                    state.typed[0] = object.msg;
                    state.typed[1] = object.excs;
                }
                Self::Syntax(object) => {
                    let object = &*object.get();
                    state.typed[..8].copy_from_slice(&[
                        object.msg,
                        object.filename,
                        object.lineno,
                        object.offset,
                        object.end_lineno,
                        object.end_offset,
                        object.text,
                        object.print_file_and_line,
                    ]);
                }
                Self::Import(object) => {
                    let object = &*object.get();
                    state.typed[..4].copy_from_slice(&[
                        object.msg,
                        object.name,
                        object.path,
                        object.name_from,
                    ]);
                }
                Self::Unicode(object) => {
                    let object = &*object.get();
                    state.typed[0] = object.encoding;
                    state.typed[1] = object.object;
                    state.typed[4] = object.reason;
                    state.unicode_start = object.start;
                    state.unicode_end = object.end;
                }
                Self::SystemExit(object) => state.typed[0] = (*object.get()).code,
                Self::OSError(object) => {
                    let object = &*object.get();
                    state.typed[0] = object.myerrno;
                    state.typed[1] = object.strerror;
                    state.typed[2] = object.filename;
                    state.typed[3] = object.filename2;
                    #[cfg(windows)]
                    {
                        state.typed[4] = object.winerror;
                    }
                    state.os_error_written = object.written;
                }
                Self::StopIteration(object) => state.typed[0] = (*object.get()).value,
                Self::NameError(object) => state.typed[0] = (*object.get()).name,
                Self::AttributeError(object) => {
                    let object = &*object.get();
                    state.typed[0] = object.obj;
                    state.typed[1] = object.name;
                }
            }
        }
        state
    }

    unsafe fn replace_state(&mut self, state: ExceptionViewState) -> Option<ExceptionViewState> {
        if state.layout_kind != self.layout_kind() {
            return None;
        }
        let old = unsafe { self.state() };
        unsafe {
            let base = self.base_mut();
            base.dict = state.base[0];
            base.args = state.base[1];
            base.notes = state.base[2];
            base.traceback = state.base[3];
            base.context = state.base[4];
            base.cause = state.base[5];
            base.suppress_context = state.suppress_context;
            match self {
                Self::Base(_) => {}
                Self::Group(object) => {
                    let object = &mut *object.get();
                    object.msg = state.typed[0];
                    object.excs = state.typed[1];
                }
                Self::Syntax(object) => {
                    let object = &mut *object.get();
                    object.msg = state.typed[0];
                    object.filename = state.typed[1];
                    object.lineno = state.typed[2];
                    object.offset = state.typed[3];
                    object.end_lineno = state.typed[4];
                    object.end_offset = state.typed[5];
                    object.text = state.typed[6];
                    object.print_file_and_line = state.typed[7];
                }
                Self::Import(object) => {
                    let object = &mut *object.get();
                    object.msg = state.typed[0];
                    object.name = state.typed[1];
                    object.path = state.typed[2];
                    object.name_from = state.typed[3];
                }
                Self::Unicode(object) => {
                    let object = &mut *object.get();
                    object.encoding = state.typed[0];
                    object.object = state.typed[1];
                    object.start = state.unicode_start;
                    object.end = state.unicode_end;
                    object.reason = state.typed[4];
                }
                Self::SystemExit(object) => (*object.get()).code = state.typed[0],
                Self::OSError(object) => {
                    let object = &mut *object.get();
                    object.myerrno = state.typed[0];
                    object.strerror = state.typed[1];
                    object.filename = state.typed[2];
                    object.filename2 = state.typed[3];
                    #[cfg(windows)]
                    {
                        object.winerror = state.typed[4];
                    }
                    object.written = state.os_error_written;
                }
                Self::StopIteration(object) => (*object.get()).value = state.typed[0],
                Self::NameError(object) => (*object.get()).name = state.typed[0],
                Self::AttributeError(object) => {
                    let object = &mut *object.get();
                    object.obj = state.typed[0];
                    object.name = state.typed[1];
                }
            }
        }
        Some(old)
    }
}

/// Physical allocation extent follows the exact class's semantic origin.
/// Static classes keep the compact prefix; HEAPTYPE always has real inline
/// protocol tables and ht_* storage before any pointer can be published.
enum ManagedTypeAllocation {
    Static(Box<UnsafeCell<PyTypeObject>>),
    Heap(Box<UnsafeCell<crate::abi_types::PyHeapTypeObject>>),
}

impl ManagedTypeAllocation {
    fn new(object: PyTypeObject) -> Self {
        if object.tp_flags & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 {
            let mut heap: crate::abi_types::PyHeapTypeObject = unsafe { std::mem::zeroed() };
            heap.ht_type = object;
            Self::Heap(Box::new(UnsafeCell::new(heap)))
        } else {
            Self::Static(Box::new(UnsafeCell::new(object)))
        }
    }

    fn heap(&self) -> Option<*mut crate::abi_types::PyHeapTypeObject> {
        match self {
            Self::Static(_) => None,
            Self::Heap(object) => Some(object.get()),
        }
    }

    fn get(&self) -> *mut PyTypeObject {
        match self {
            Self::Static(object) => object.get(),
            Self::Heap(object) => object.get().cast(),
        }
    }
}

impl Drop for ManagedTypeAllocation {
    fn drop(&mut self) {
        // Box retirement is the final address-reuse boundary, including an
        // unpublished rejection and publication rollback. Reentrant edge
        // releases may have observed the still-live allocation again.
        crate::api::typeobj::unregister_type_address(self.get().addr());
    }
}

enum ManagedView {
    Object(Box<BridgeHeader>),
    /// Concrete `PyCFunctionObject`/`PyCMethodObject` callable layout. A plain
    /// function uses the method allocation with a null `mm_class`, so one
    /// release path owns every C member edge.
    CFunction(Box<UnsafeCell<PyCMethodObject>>),
    /// Runtime-defined builtin callable: real vectorcall storage at the public
    /// callable offset, but no invented native PyMethodDef or C receiver.
    /// Physical MoltManaged_Type keeps native introspection distinct.
    RuntimeCallable(Box<UnsafeCell<PyCMethodObject>>),
    Type {
        object: ManagedTypeAllocation,
        _name: std::ffi::CString,
    },
    Tuple {
        allocation: TupleAllocation,
    },
    List {
        allocation: ListAllocation,
    },
    Exception(ExceptionAllocation),
    Slice(Box<UnsafeCell<crate::abi_types::PySliceObject>>),
    MemoryView {
        object: Box<UnsafeCell<crate::abi_types::PyMemoryViewObject>>,
        format: std::ffi::CString,
    },
}

unsafe impl Send for ManagedView {}

impl ManagedView {
    fn py_obj(&self) -> *mut PyObject {
        match self {
            Self::Object(header) => header.py_obj.get(),
            Self::CFunction(object) => object.get().cast::<PyObject>(),
            Self::RuntimeCallable(object) => object.get().cast::<PyObject>(),
            Self::Type { object, .. } => object.get().cast::<PyObject>(),
            Self::Slice(object) => object.get().cast::<PyObject>(),
            Self::Tuple { allocation, .. } => allocation.py_obj(),
            Self::List { allocation, .. } => allocation.py_obj(),
            Self::Exception(allocation) => allocation.py_obj(),
            Self::MemoryView { object, .. } => object.get().cast(),
        }
    }

    /// Release references owned solely by a concrete-layout sidecar.  This is
    /// deliberately called only after bridge-map locks have been dropped:
    /// carrier deallocation re-enters the address registry.
    fn release_owned_items(&mut self) {
        crate::api::errors::with_preserved_error(|| {
            self.release_owned_items_with(|pointer, mirrored| unsafe {
                if mirrored {
                    GLOBAL_BRIDGE.projection_decref(pointer);
                } else {
                    crate::api::refcount::Py_XDECREF(pointer);
                }
            });
        });
    }

    /// One physical-owner inventory serves discovery and retirement. Discovery
    /// never changes a field; retirement publishes its empty value before any
    /// release callback. Alias slots share one owner, not two decrefs.
    fn owned_items_with(
        &mut self,
        detach: bool,
        type_cycle: bool,
        mut visit: impl FnMut(*mut PyObject, bool),
    ) {
        unsafe fn field(slot: *mut *mut PyObject, detach: bool) -> *mut PyObject {
            unsafe {
                if detach {
                    slot.replace(std::ptr::null_mut())
                } else {
                    slot.read()
                }
            }
        }
        match self {
            Self::Slice(object) => unsafe {
                let object = object.get();
                let fields = [
                    field(&raw mut (*object).start, detach),
                    field(&raw mut (*object).stop, detach),
                    field(&raw mut (*object).step, detach),
                ];
                for pointer in fields {
                    visit(pointer, true);
                }
            },
            Self::Tuple { allocation } => unsafe {
                for index in 0..allocation.len {
                    visit(field(allocation.items_ptr().add(index), detach), true);
                }
            },
            Self::List { allocation } => unsafe {
                for index in 0..allocation.items.len() {
                    let current = field(allocation.items.as_mut_ptr().add(index), detach);
                    let shadow = field(allocation.shadow.as_mut_ptr().add(index), detach);
                    visit(shadow, true);
                    if current != shadow {
                        visit(current, false);
                    }
                }
            },
            Self::Exception(allocation) => unsafe {
                let state = if detach {
                    allocation
                        .replace_state(ExceptionViewState::empty(allocation.layout_kind()))
                        .expect("same exception allocation layout")
                } else {
                    allocation.state()
                };
                for pointer in state.pointers() {
                    visit(pointer, false);
                }
            },
            Self::CFunction(object) => unsafe {
                let object = object.get();
                let receiver = field(&raw mut (*object).func.m_self, detach);
                let module = field(&raw mut (*object).func.m_module, detach);
                let class = field(
                    (&raw mut (*object).mm_class).cast::<*mut PyObject>(),
                    detach,
                );
                visit(receiver, true);
                visit(module, false);
                visit(class, true);
            },
            Self::Type { object, .. } => unsafe {
                if detach && !type_cycle {
                    crate::api::typeobj::unregister_type_address(object.get().addr());
                }
                let heap = object.heap();
                let object = object.get();
                let roots = [
                    field(&raw mut (*object).tp_dict, detach),
                    if type_cycle {
                        std::ptr::null_mut()
                    } else {
                        field(&raw mut (*object).tp_bases, detach)
                    },
                    field(&raw mut (*object).tp_mro, detach),
                ];
                let names = if let Some(heap) = heap.filter(|_| !type_cycle) {
                    [
                        field(&raw mut (*heap).ht_name, detach),
                        field(&raw mut (*heap).ht_qualname, detach),
                    ]
                } else {
                    [std::ptr::null_mut(); 2]
                };
                for root in roots.into_iter().chain(names) {
                    visit(root, true);
                }
            },
            Self::Object(_) | Self::RuntimeCallable(_) | Self::MemoryView { .. } => {}
        }
    }

    /// Empty each physical owner before releasing it. Publication rollback and
    /// interpreter retirement share this inventory and keep every member live.
    fn release_owned_items_with(&mut self, release: impl FnMut(*mut PyObject, bool)) {
        self.owned_items_with(true, false, release);
    }
}

thread_local! {
    /// Exception state crosses the crate boundary through hooks that can
    /// materialize lazy args/tracebacks and therefore re-enter the bridge.
    /// Suppress recursion only for the exception already being synchronized.
    /// A distinct nested exception must still publish its complete physical
    /// `PyBaseExceptionObject`; a thread-global depth bit left those nested
    /// views permanently initialized with null fields.
    static EXCEPTION_SYNC_STACK: RefCell<ReentrantHandleStack> = const {
        RefCell::new(ReentrantHandleStack::new())
    };
    static LIST_SYNC_STACK: RefCell<ReentrantHandleStack> = const {
        RefCell::new(ReentrantHandleStack::new())
    };
    /// Only the building owner may observe its recursive projection skeleton;
    /// the inline stack keeps the common publication path allocation-free.
    static PUBLICATION_BUILD_STACK: RefCell<PublicationBuildStack> = const {
        RefCell::new(PublicationBuildStack::new())
    };
}

const REENTRANT_HANDLE_INLINE_DEPTH: usize = 8;

struct ReentrantHandleStack {
    inline: [AbiHandle; REENTRANT_HANDLE_INLINE_DEPTH],
    depth: usize,
    overflow: Vec<AbiHandle>,
}

impl ReentrantHandleStack {
    const fn new() -> Self {
        Self {
            inline: [0; REENTRANT_HANDLE_INLINE_DEPTH],
            depth: 0,
            overflow: Vec::new(),
        }
    }

    fn contains(&self, bits: AbiHandle) -> bool {
        self.inline[..self.depth.min(REENTRANT_HANDLE_INLINE_DEPTH)].contains(&bits)
            || self.overflow.contains(&bits)
    }

    fn push(&mut self, bits: AbiHandle) {
        if self.depth < REENTRANT_HANDLE_INLINE_DEPTH {
            self.inline[self.depth] = bits;
        } else {
            self.overflow.push(bits);
        }
        self.depth += 1;
    }

    fn pop(&mut self) -> Option<AbiHandle> {
        let next_depth = self.depth.checked_sub(1)?;
        self.depth = next_depth;
        if next_depth < REENTRANT_HANDLE_INLINE_DEPTH {
            Some(std::mem::replace(&mut self.inline[next_depth], 0))
        } else {
            self.overflow.pop()
        }
    }
}

struct ExceptionSyncGuard {
    bits: AbiHandle,
}

impl ExceptionSyncGuard {
    fn enter(bits: AbiHandle) -> Option<Self> {
        EXCEPTION_SYNC_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.contains(bits) {
                None
            } else {
                stack.push(bits);
                Some(Self { bits })
            }
        })
    }
}

impl Drop for ExceptionSyncGuard {
    fn drop(&mut self) {
        EXCEPTION_SYNC_STACK.with(|stack| {
            let popped = stack.borrow_mut().pop();
            debug_assert_eq!(popped, Some(self.bits));
        });
    }
}

struct ListSyncGuard {
    bits: AbiHandle,
}

impl ListSyncGuard {
    fn enter(bits: AbiHandle) -> Option<Self> {
        LIST_SYNC_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.contains(bits) {
                None
            } else {
                stack.push(bits);
                Some(Self { bits })
            }
        })
    }
}

impl Drop for ListSyncGuard {
    fn drop(&mut self) {
        LIST_SYNC_STACK.with(|stack| {
            let popped = stack.borrow_mut().pop();
            debug_assert_eq!(popped, Some(self.bits));
        });
    }
}

fn release_bridge_entry(mut entry: BridgeEntry) {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let _ = unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(entry.bits, 0) };
    entry.view.release_owned_items();
}

/// A canonical projection removed from both bridge identity maps but kept
/// alive until the runtime has published every semantic edge source empty.
pub struct RetiredRuntimeView {
    entry: Option<Box<BridgeEntry>>,
}

impl Drop for RetiredRuntimeView {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            release_bridge_entry(*entry);
        }
    }
}

struct BridgeEntry {
    view: ManagedView,
    bits: AbiHandle,
    /// The sole object-owned Unicode construction/export projection. It owns
    /// the codepoint data and optional strict UTF-8 encoding cache together.
    unicode: Option<UnicodeProjection>,
    publication: PublicationState,
    lifecycle: BridgeLifecycle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PublicationState {
    Building { owner: std::thread::ThreadId },
    Ready,
    Retiring,
}

/// The DFS stack is also the custody of provisional publications. `dependency`
/// is the oldest still-building view observed by a frame or its descendants.
/// A recursive component commits together; independent metadata and exception
/// projections can finish without becoming part of an outer rollback.
struct PublicationFrame {
    bridge: *const ObjectBridge,
    bits: AbiHandle,
    pointer: *mut PyObject,
    dependency: usize,
    complete: bool,
    failed: bool,
    committed: bool,
    pin_released: bool,
    aborting: bool,
    edges_cleared: bool,
    retired: Option<Box<BridgeEntry>>,
}

struct PublicationBuildStack {
    frames: Vec<PublicationFrame>,
    active: Vec<usize>,
    rolling_back: bool,
}

impl PublicationBuildStack {
    const fn new() -> Self {
        Self {
            frames: Vec::new(),
            active: Vec::new(),
            rolling_back: false,
        }
    }

    fn find(&self, bridge: &ObjectBridge, bits: AbiHandle) -> Option<usize> {
        self.frames.iter().position(|frame| {
            std::ptr::eq(frame.bridge, bridge)
                && frame.bits == bits
                && (!frame.committed || !frame.pin_released)
        })
    }
}

struct PublicationBuildGuard<'a> {
    bridge: &'a ObjectBridge,
    index: usize,
    resolved: bool,
}

impl<'a> PublicationBuildGuard<'a> {
    /// Enter before metadata construction: resolving a base or an exception's
    /// class can itself construct views before this frame has a C allocation.
    fn enter(bridge: &'a ObjectBridge, bits: AbiHandle) -> Option<Self> {
        let admission = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.find(bridge, bits).is_some() {
                return Err(false);
            }
            if stack.frames.try_reserve(1).is_err() || stack.active.try_reserve(1).is_err() {
                return Err(true);
            }
            let index = stack.frames.len();
            stack.frames.push(PublicationFrame {
                bridge,
                bits,
                pointer: std::ptr::null_mut(),
                dependency: index,
                complete: false,
                failed: false,
                committed: false,
                pin_released: false,
                aborting: false,
                edges_cleared: false,
                retired: None,
            });
            stack.active.push(index);
            Ok(index)
        });
        match admission {
            Ok(index) => Some(Self {
                bridge,
                index,
                resolved: false,
            }),
            Err(no_memory) => {
                unsafe {
                    if no_memory {
                        crate::api::errors::PyErr_NoMemory();
                    } else {
                        ensure_result_error(c"recursive ABI metadata has no publication skeleton");
                    }
                }
                None
            }
        }
    }

    fn inserted(&self, pointer: *mut PyObject) {
        PUBLICATION_BUILD_STACK
            .with(|stack| stack.borrow_mut().frames[self.index].pointer = pointer);
    }

    fn finish(mut self) -> bool {
        self.resolve(true)
    }

    fn resolve(&mut self, success: bool) -> bool {
        self.resolved = true;
        let (success, commit, outermost) = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let success = success && !stack.frames[self.index].failed;
            stack.frames[self.index].complete = success;
            if !success {
                for index in 0..stack.active.len() {
                    let active = stack.active[index];
                    stack.frames[active].failed = true;
                }
            }
            assert_eq!(stack.active.pop(), Some(self.index));
            let dependency = stack.frames[self.index].dependency;
            if let Some(&parent) = stack.active.last() {
                stack.frames[parent].dependency = stack.frames[parent].dependency.min(dependency);
            }
            (
                success,
                success && dependency == self.index,
                self.index == 0 && !stack.rolling_back,
            )
        });
        if commit {
            self.bridge.commit_publication_component(self.index);
        }
        if outermost {
            crate::api::errors::with_preserved_error(|| self.bridge.finish_publication_stack());
        }
        success
    }
}

impl Drop for PublicationBuildGuard<'_> {
    fn drop(&mut self) {
        if !self.resolved {
            self.resolve(false);
        }
    }
}

#[inline]
fn is_publication_owner(
    bridge: &ObjectBridge,
    bits: AbiHandle,
    owner: &std::thread::ThreadId,
) -> bool {
    *owner == std::thread::current().id()
        && PUBLICATION_BUILD_STACK.with(|stack| {
            let stack = stack.borrow();
            // Every Building identity is minted by this thread's transaction
            // stack. During its closed edge drain, decrefs need only that owner
            // proof; rescanning the component for each edge is quadratic.
            stack.rolling_back || stack.find(bridge, bits).is_some()
        })
}

/// Record a dependency only when a provisional pointer is observed/retained,
/// not when an edge is released. Return false once rollback has closed it.
fn observe_publication(bridge: &ObjectBridge, bits: AbiHandle) -> bool {
    PUBLICATION_BUILD_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(target) = stack.find(bridge, bits) else {
            return true;
        };
        if stack.frames[target].aborting {
            return false;
        }
        if stack.frames[target].committed {
            return true;
        }
        if let Some(&current) = stack.active.last() {
            let dependency = stack.frames[target].dependency;
            stack.frames[current].dependency = stack.frames[current].dependency.min(dependency);
        }
        true
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeLifecycle {
    /// Only the canonical view's stable runtime hold remains. `ob_refcnt`
    /// contains direct C references only.
    ViewHoldOnly,
    /// At least one non-view runtime owner exists. `ob_refcnt` includes one
    /// borrowed-view bias in addition to direct C references.
    RuntimeOwned,
    /// The stable view hold and a distinct runtime finalizer pin are live.
    /// `ob_refcnt` includes the matching finalizer bias. Ordinary runtime-owner
    /// 1<->2 transitions are suppressed until the window resolves.
    FinalizingPin,
}

/// Result of one header release linearized with its canonical ABI-view
/// lifecycle. `should_finalize` means the bridge has already published the
/// `FinalizingPin` gate, so no later non-owning runtime upgrade can reopen the
/// view-hold-only baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeOwnerRelease {
    previous: u32,
    should_finalize: bool,
}

impl RuntimeOwnerRelease {
    #[inline]
    pub const fn previous(self) -> u32 {
        self.previous
    }

    #[inline]
    pub const fn should_finalize(self) -> bool {
        self.should_finalize
    }
}

impl BridgeLifecycle {
    #[inline]
    fn has_c_bias(self) -> bool {
        matches!(self, Self::RuntimeOwned | Self::FinalizingPin)
    }
}

/// Project runtime ownership into the initial C header. Shape and physical
/// storage do not determine lifetime: an empty tuple subclass is mortal, while
/// canonical strings and tuples inherit their runtime owner's immortality.
#[inline]
fn initial_managed_view_refs(runtime_refs: usize, owned: bool) -> (isize, bool) {
    let has_runtime_owner = !owned || runtime_refs > 1;
    let c_refs = if runtime_refs == molt_codegen_abi::IMMORTAL_REFCOUNT as usize {
        crate::abi_types::IMMORTAL_REFCNT
    } else {
        isize::from(owned) + isize::from(has_runtime_owner)
    };
    (c_refs, has_runtime_owner)
}

/// C immortality is a lifetime state, not a large count from which a runtime
/// bias or mirrored edge may be subtracted. C API promotion and runtime-owned
/// canonical objects share this interpretation of the physical header.
#[derive(Clone, Copy)]
enum CReferenceCount {
    Counted(isize),
    Immortal,
}

impl CReferenceCount {
    fn read(refs: isize, operation: &str, lifecycle: BridgeLifecycle) -> Self {
        if refs < 0 {
            abort_refcount_invariant(operation, refs, lifecycle);
        }
        if crate::abi_types::is_immortal_refcnt(refs) {
            Self::Immortal
        } else {
            Self::Counted(refs)
        }
    }

    fn without_bias(self, has_bias: bool, operation: &str, lifecycle: BridgeLifecycle) -> Self {
        match self {
            Self::Immortal => Self::Immortal,
            Self::Counted(refs) => Self::Counted(
                checked_c_refs_without_bias(refs, has_bias)
                    .unwrap_or_else(|| abort_refcount_invariant(operation, refs, lifecycle)),
            ),
        }
    }
}

impl BridgeEntry {
    fn change_c_bias(&self, add: bool, operation: &str) {
        let ptr = self.view.py_obj();
        let refs = unsafe { (*ptr).ob_refcnt };
        if let CReferenceCount::Counted(refs) =
            CReferenceCount::read(refs, operation, self.lifecycle)
        {
            let updated = if add {
                checked_c_ref_increment(refs)
            } else {
                checked_c_refs_without_bias(refs, true)
            }
            .unwrap_or_else(|| abort_refcount_invariant(operation, refs, self.lifecycle));
            unsafe { (*ptr).ob_refcnt = updated };
        }
    }
}

#[inline]
fn checked_c_refs_without_bias(refs: isize, has_bias: bool) -> Option<isize> {
    if refs < 0 || crate::abi_types::is_immortal_refcnt(refs) {
        return None;
    }
    refs.checked_sub(isize::from(has_bias))
        .filter(|direct| *direct >= 0)
}

#[inline]
fn checked_c_ref_increment(refs: isize) -> Option<isize> {
    if refs < 0 || crate::abi_types::is_immortal_refcnt(refs) {
        None
    } else {
        refs.checked_add(1)
    }
}

#[cold]
fn abort_refcount_invariant(operation: &str, refs: isize, lifecycle: BridgeLifecycle) -> ! {
    eprintln!(
        "molt fatal: canonical ABI refcount invariant failed during {operation}: refs={refs} lifecycle={lifecycle:?}"
    );
    std::process::abort()
}

/// Global bridge, one per process (extensions are global singletons).
pub static GLOBAL_BRIDGE: once_cell::sync::Lazy<ObjectBridge> =
    once_cell::sync::Lazy::new(ObjectBridge::new);

struct AddressShard {
    from_py: HashMap<usize, AbiHandle>,
    direct_molt_py: HashMap<usize, AbiHandle>,
    numeric_carriers: HashMap<usize, NumericCarrierRecord>,
    /// Private mirrored C references already represented by runtime graph edges.
    /// Independently traversed, C-writable fields use ordinary C ownership and
    /// must never enter this ledger: direct writes can replace them without a
    /// bridge operation.
    projection_refs: HashMap<usize, usize>,
    foreign: HashMap<usize, AbiHandle>,
    foreign_inflight: HashSet<usize>,
}

#[derive(Clone, Copy)]
pub(crate) enum NumericCarrierKind {
    Long { allocation_size: usize },
    Float,
    Complex,
}

#[derive(Clone, Copy)]
pub(crate) struct NumericCarrierRecord {
    pub bits: Option<AbiHandle>,
    pub kind: NumericCarrierKind,
}

/// Raw reverse identities are borrowed from another lifetime authority
/// (foreign wrappers and static bindings) and never own a runtime reference.
/// Runtime-backed ABI callables are canonical managed views instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RawBinding {
    address: usize,
}

impl RawBinding {
    fn borrowed(address: usize) -> Self {
        Self { address }
    }
}

struct HandleShard {
    to_py: HashMap<AbiHandle, Box<BridgeEntry>>,
    raw_py: HashMap<AbiHandle, RawBinding>,
}

/// One semantic crossing's runtime custody. Existing canonical values are
/// borrowed from the caller's C owner; foreign wrappers transfer one temporary
/// runtime reference, released only after consumers acquire their own edges.
pub(crate) struct RuntimeValue {
    bits: AbiHandle,
    owned: bool,
}

#[derive(Clone, Copy)]
enum RuntimeValueAccess {
    Observe,
    RetainEdge,
}

impl RuntimeValue {
    /// The caller keeps `object` alive until this guard is dropped. Null is not
    /// a Python value; optional arguments must choose their None value explicitly.
    pub(crate) unsafe fn acquire(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        unsafe { GLOBAL_BRIDGE.acquire_runtime_value(object, RuntimeValueAccess::Observe) }
    }

    /// Retain a reference edge without requiring a container's construction to
    /// be complete. Initialized direct-C slots and layout validity still commit
    /// through the same authority; only semantic reads require every slot.
    pub(crate) unsafe fn acquire_edge(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        unsafe { GLOBAL_BRIDGE.acquire_runtime_value(object, RuntimeValueAccess::RetainEdge) }
    }

    pub(crate) fn bits(&self) -> AbiHandle {
        self.bits
    }

    /// Adopt one already-owned runtime result. The caller must have validated
    /// the producing API's failure sentinel (in particular zero for handle-only
    /// allocation hooks); this constructor does not create or validate a value.
    pub(crate) unsafe fn from_owned(bits: AbiHandle) -> Self {
        Self { bits, owned: true }
    }

    pub(crate) fn into_owned_bits(mut self) -> AbiHandle {
        if !self.owned {
            unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(self.bits) };
        }
        self.owned = false;
        self.bits
    }
}

impl Drop for RuntimeValue {
    fn drop(&mut self) {
        if self.owned {
            crate::api::errors::with_preserved_error(|| unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(self.bits);
            });
        }
    }
}

impl HandleShard {
    // Managed ownership is an exact physical address, not an ingress-map
    // property. A direct ingress hint may also exist for this managed view,
    // while independent borrowed aliases can share the same runtime handle.
    fn managed_address(&self, bits: AbiHandle) -> Option<usize> {
        self.to_py
            .get(&bits)
            .map(|entry| entry.view.py_obj().addr())
    }
}

/// Static publication failures captured at the locked admission boundary.
/// The facts describe the rejected transaction, not a later diagnostic lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticBindingError {
    InvalidInput {
        address: usize,
        bits: AbiHandle,
    },
    AddressIdentityConflict {
        forward: Option<AbiHandle>,
        direct: Option<AbiHandle>,
        foreign: Option<AbiHandle>,
        foreign_inflight: bool,
        numeric_carrier: Option<Option<AbiHandle>>,
    },
    CanonicalTargetManaged {
        address: usize,
    },
    CanonicalTargetBorrowed {
        address: usize,
    },
    ManagedPrevious {
        bits: AbiHandle,
        address: usize,
    },
}

/// Why a fully built managed entry could not enter the identity maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagedEntryRejection {
    Occupied,
    NoMemory,
    Deallocating,
}

impl ManagedEntryRejection {
    unsafe fn set_error(self) {
        match self {
            Self::Occupied => unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"ABI view runtime identity was already published".as_ptr(),
                )
            },
            Self::NoMemory => unsafe {
                crate::api::errors::PyErr_NoMemory();
            },
            Self::Deallocating => unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RuntimeError).cast::<PyObject>(),
                    c"cannot publish ABI view for deallocating object".as_ptr(),
                )
            },
        }
    }
}

/// Storage authority selected when a C-visible object reaches its release
/// boundary.  This is deliberately not a boolean: managed views, registered
/// direct objects, numeric carriers, immortal statics, and unknown foreign
/// objects have distinct destruction obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyObjRelease {
    ManagedViewRetired,
    DirectViewUnregistered,
    NumericCarrier,
    StaticImmortal,
    Untracked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedDecref {
    NotManaged,
    Immortal,
    Alive,
    RetiredInline,
    ReleaseRuntimeHold(AbiHandle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CRefZero {
    ViewRetained,
    ReleaseRuntimeHold,
}

impl PyObjRelease {
    #[inline]
    pub const fn requires_type_dealloc(self) -> bool {
        matches!(
            self,
            Self::DirectViewUnregistered | Self::NumericCarrier | Self::Untracked
        )
    }
}

/// Sharded global bridge state. Address-keyed identity maps and handle-keyed
/// value maps have distinct lock ranks. Every operation needing both ranks
/// acquires the address shard first, then distinct handle shards in index order.
pub struct ObjectBridge {
    address_shards: Box<[Mutex<AddressShard>]>,
    foreign_ready: Box<[Condvar]>,
    handle_shards: Box<[Mutex<HandleShard>]>,
    publication_ready: Box<[Condvar]>,
    shard_mask: usize,
}

pub(crate) unsafe fn ensure_result_error(message: &std::ffi::CStr) {
    if !crate::api::errors::raised_error_pending() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                message.as_ptr(),
            );
        }
    }
}

#[cfg(target_arch = "wasm32")]
unsafe impl Sync for ObjectBridge {}

/// SIMD tag→type lookup table.
/// Index is `MoltTypeTag as u8`, value is `*mut PyTypeObject`.
/// Fits in exactly 16 entries (one SIMD lane on SSE/NEON).
struct TypeTagTable {
    tags: [u8; 16],
    types: [*mut PyTypeObject; 16],
    len: usize,
}

unsafe impl Send for TypeTagTable {}
unsafe impl Sync for TypeTagTable {}

static TAG_TABLE: OnceCell<TypeTagTable> = OnceCell::new();

/// Build the tag table once at init time.
pub fn init_tag_table() {
    TAG_TABLE.get_or_init(|| {
        let mut table = TypeTagTable {
            tags: [0u8; 16],
            types: [std::ptr::null_mut(); 16],
            len: 0,
        };
        macro_rules! push {
            ($tag:expr, $ty:expr) => {{
                let i = table.len;
                table.tags[i] = $tag as u8;
                table.types[i] = &raw mut $ty;
                table.len += 1;
            }};
        }
        // `None` never reaches proxy allocation (the `Py_None` singleton path
        // resolves first), so it does not consume a SIMD lane.
        push!(MoltTypeTag::Bool, PyBool_Type);
        push!(MoltTypeTag::Int, MoltManaged_Type);
        push!(MoltTypeTag::Float, MoltManaged_Type);
        push!(MoltTypeTag::Complex, MoltManaged_Type);
        push!(MoltTypeTag::Str, MoltManaged_Type);
        push!(MoltTypeTag::Bytes, MoltManaged_Type);
        // Lists have a real CPython-layout sidecar. Publishing the generic
        // managed type here would make a truthful `PyListObject` allocation
        // advertise the wrong physical type and would defeat direct Cython
        // `ob_item` access.
        push!(MoltTypeTag::List, PyList_Type);
        push!(MoltTypeTag::Tuple, PyTuple_Type);
        push!(MoltTypeTag::Dict, MoltManaged_Type);
        push!(MoltTypeTag::Set, MoltManaged_Type);
        push!(MoltTypeTag::FrozenSet, MoltManaged_Type);
        push!(MoltTypeTag::Type, PyType_Type);
        push!(MoltTypeTag::Module, MoltManaged_Type);
        // Traceback's public struct has a frame/next/lineno tail. Until that
        // entire sidecar exists, expose only the honest generic managed view.
        push!(MoltTypeTag::Traceback, MoltManaged_Type);
        // Exception views replace this fallback with the instance's exact
        // runtime class while building the canonical view.
        push!(MoltTypeTag::Exception, MoltManaged_Type);
        // `Other` covers every Molt heap type without a dedicated static type
        // (functions, classes, bound methods, arbitrary instances). It MUST NOT
        // masquerade as a concrete builtin: mapping it to `PyUnicode_Type` made
        // a Molt-compiled function proxy fail `PyObject_Call` with the lying
        // diagnostic "'str' object is not callable" (numpy `_multiarray_umath`
        // init calling `numpy.dtypes._add_dtype_helper`). `PyBaseObject_Type`
        // ("object") is the honest neutral: no `tp_call`, no false type checks.
        push!(MoltTypeTag::Other, MoltManaged_Type);
        table
    });
}

/// Resolve a Molt type tag to its static `PyTypeObject*` using the fastest
/// available SIMD instruction set.
///
/// # Safety
/// `init_tag_table()` must have been called before first use.
#[inline]
pub unsafe fn tag_to_type(tag: MoltTypeTag) -> *mut PyTypeObject {
    if tag == MoltTypeTag::Slice {
        return &raw mut crate::abi_types::PySlice_Type;
    }
    if tag == MoltTypeTag::MemoryView {
        return &raw mut crate::abi_types::PyMemoryView_Type;
    }
    // Runtime callable storage has a vectorcall tail, not a native C method
    // definition. It shares the generic physical discriminator.
    let tag = if tag == MoltTypeTag::BuiltinCallable {
        MoltTypeTag::Other
    } else {
        tag
    };
    let needle = tag as u8;

    #[cfg(all(target_arch = "x86_64", feature = "simd"))]
    unsafe {
        return simd_x86::lookup_type(needle);
    }

    #[cfg(all(target_arch = "aarch64", feature = "simd"))]
    unsafe {
        return simd_neon::lookup_type(needle);
    }

    // Scalar fallback — 16-entry linear scan, branch predictor handles well.
    #[allow(unreachable_code)]
    {
        let table = TAG_TABLE.get().expect("init_tag_table not called");
        for i in 0..table.len {
            if table.tags[i] == needle {
                return table.types[i];
            }
        }
        // SAFETY: PyBaseObject_Type is a valid static with the same lifetime as the program.
        &raw mut PyBaseObject_Type
    }
}

#[cfg(all(target_arch = "x86_64", feature = "simd"))]
mod simd_x86 {
    use super::*;
    use std::arch::x86_64::*;

    /// SSE4.1 path: compare 16 tag bytes in one instruction.
    #[target_feature(enable = "sse4.1")]
    pub unsafe fn lookup_type(needle: u8) -> *mut PyTypeObject {
        let table = TAG_TABLE.get().expect("init_tag_table not called");

        let tags_vec = unsafe { _mm_loadu_si128(table.tags.as_ptr().cast()) };
        let needle_vec = unsafe { _mm_set1_epi8(needle as i8) };
        let cmp = unsafe { _mm_cmpeq_epi8(tags_vec, needle_vec) };
        let mask = unsafe { _mm_movemask_epi8(cmp) } as u32;

        if mask != 0 {
            let idx = mask.trailing_zeros() as usize;
            if idx < table.len {
                return table.types[idx];
            }
        }
        &raw mut PyBaseObject_Type
    }
}

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
mod simd_neon {
    use super::*;
    use std::arch::aarch64::*;

    /// NEON path: vceqq_u8 + first-set-bit extraction.
    pub unsafe fn lookup_type(needle: u8) -> *mut PyTypeObject {
        let table = TAG_TABLE.get().expect("init_tag_table not called");

        let tags_vec = unsafe { vld1q_u8(table.tags.as_ptr()) };
        let needle_vec = unsafe { vdupq_n_u8(needle) };
        let cmp = unsafe { vceqq_u8(tags_vec, needle_vec) };

        // Extract match positions via u64 lanes.
        let lo = unsafe { vgetq_lane_u64(vreinterpretq_u64_u8(cmp), 0) };
        let hi = unsafe { vgetq_lane_u64(vreinterpretq_u64_u8(cmp), 1) };

        let idx = if lo != 0 {
            lo.trailing_zeros() as usize / 8
        } else if hi != 0 {
            8 + hi.trailing_zeros() as usize / 8
        } else {
            return &raw mut PyBaseObject_Type;
        };

        if idx < table.len {
            table.types[idx]
        } else {
            &raw mut PyBaseObject_Type
        }
    }
}

impl ObjectBridge {
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let shard_count = 1usize;
        #[cfg(not(target_arch = "wasm32"))]
        let shard_count = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .saturating_mul(2)
            .next_power_of_two();

        let address_shards = (0..shard_count)
            .map(|_| {
                Mutex::new(AddressShard {
                    from_py: HashMap::new(),
                    direct_molt_py: HashMap::new(),
                    numeric_carriers: HashMap::new(),
                    projection_refs: HashMap::new(),
                    foreign: HashMap::new(),
                    foreign_inflight: HashSet::new(),
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let handle_shards = (0..shard_count)
            .map(|_| {
                Mutex::new(HandleShard {
                    to_py: HashMap::new(),
                    raw_py: HashMap::new(),
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let foreign_ready = (0..shard_count)
            .map(|_| Condvar::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let publication_ready = (0..shard_count)
            .map(|_| Condvar::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self {
            address_shards,
            foreign_ready,
            handle_shards,
            publication_ready,
            shard_mask: shard_count - 1,
        }
    }

    #[inline(always)]
    fn address_shard_index(&self, addr: usize) -> usize {
        (addr >> 4) & self.shard_mask
    }

    #[inline(always)]
    fn handle_shard_index(&self, bits: AbiHandle) -> usize {
        ((bits >> 4) as usize) & self.shard_mask
    }

    #[inline(always)]
    fn address_shard(&self, addr: usize) -> &Mutex<AddressShard> {
        unsafe {
            self.address_shards
                .get_unchecked(self.address_shard_index(addr))
        }
    }

    #[inline(always)]
    fn handle_shard(&self, bits: AbiHandle) -> &Mutex<HandleShard> {
        unsafe {
            self.handle_shards
                .get_unchecked(self.handle_shard_index(bits))
        }
    }

    #[inline]
    fn lock_address_then_handle(
        &self,
        addr: usize,
        bits: AbiHandle,
    ) -> (MutexGuard<'_, AddressShard>, MutexGuard<'_, HandleShard>) {
        let address = self.address_shard(addr).lock();
        let handle = self.handle_shard(bits).lock();
        (address, handle)
    }

    #[cfg(test)]
    fn shard_count(&self) -> usize {
        self.address_shards.len()
    }

    fn singleton_pyobj(bits: AbiHandle) -> Option<*mut PyObject> {
        let obj = MoltObject::from_bits(bits);
        if obj.is_none() {
            return Some(&raw mut Py_None);
        }
        if obj.is_bool() {
            return Some(if obj.as_bool().unwrap_or(false) {
                (&raw mut Py_True).cast::<PyObject>()
            } else {
                (&raw mut Py_False).cast::<PyObject>()
            });
        }
        obj.as_int()
            .and_then(crate::api::numbers::cached_small_int_ptr)
    }

    unsafe fn increment_pyobj_ref(ptr: *mut PyObject) {
        unsafe {
            if !crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) {
                (*ptr).ob_refcnt += 1;
            }
        }
    }

    unsafe fn managed_type_metadata(
        bits: AbiHandle,
    ) -> Option<(std::ffi::CString, Option<u64>, std::os::raw::c_ulong)> {
        use crate::hooks::{DecodedHandleResult, TypeMetadataField};
        let hooks = crate::hooks::hooks_or_stubs();
        let name_bits =
            match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::Name) }.decode() {
                DecodedHandleResult::Ok(name) => name,
                DecodedHandleResult::Missing | DecodedHandleResult::Error => {
                    unsafe { ensure_result_error(c"managed type name unavailable") };
                    return None;
                }
            };
        let mut length = 0usize;
        let bytes = unsafe { (hooks.str_data)(name_bits, &raw mut length) };
        let name = if bytes.is_null() {
            None
        } else {
            // Heap tp_name is the current __name__, not module/qualname. Keep
            // WTF-8 intact; constructing this projection must not run Python.
            std::ffi::CString::new(unsafe { std::slice::from_raw_parts(bytes, length) }).ok()
        };
        unsafe { (hooks.dec_ref)(name_bits) };
        let Some(name) = name else {
            unsafe { ensure_result_error(c"invalid managed type name") };
            return None;
        };
        let flags_bits =
            match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::SemanticFlags) }.decode()
            {
                DecodedHandleResult::Ok(flags) => flags,
                _ => {
                    unsafe { ensure_result_error(c"managed type semantic flags unavailable") };
                    return None;
                }
            };
        let flags = MoltObject::from_bits(flags_bits).as_int();
        unsafe { (hooks.dec_ref)(flags_bits) };
        let Some(flags) = flags.filter(|value| *value >= 0) else {
            unsafe { ensure_result_error(c"invalid managed type semantic flags") };
            return None;
        };
        let base_bits =
            match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::Base) }.decode() {
                DecodedHandleResult::Ok(base) => Some(base),
                DecodedHandleResult::Missing => None,
                DecodedHandleResult::Error => {
                    unsafe { ensure_result_error(c"managed type base unavailable") };
                    return None;
                }
            };
        Some((name, base_bits, flags as std::os::raw::c_ulong))
    }
}

// Fallible physical-view construction prior to publication.
impl ObjectBridge {
    /// Resolve the runtime's borrowed class identity through the one bridge
    /// projection authority. Static builtin bindings and managed Type views
    /// both come from `handle_to_borrowed_pyobj`; neither changes the physical
    /// storage type of the value whose class is requested.
    unsafe fn runtime_class_view(&self, bits: u64) -> *mut PyTypeObject {
        let result = unsafe { (crate::hooks::hooks_or_stubs().runtime_class_borrowed)(bits) };
        let class_bits = match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(class_bits) if class_bits != 0 => class_bits,
            _ => {
                unsafe { ensure_result_error(c"managed runtime class identity unavailable") };
                return std::ptr::null_mut();
            }
        };
        let view = unsafe { self.handle_to_borrowed_pyobj(class_bits) };
        if view.is_null() {
            unsafe { ensure_result_error(c"managed runtime class has no ABI Type view") };
        }
        view.cast::<PyTypeObject>()
    }

    unsafe fn build_pyobj_entry(
        &self,
        bits: AbiHandle,
        ob_refcnt: isize,
        internal_c_ref: bool,
    ) -> Option<(Box<BridgeEntry>, *mut PyObject)> {
        let tag = Self::classify_handle(bits);
        let ob_type = if tag == MoltTypeTag::Exception {
            let class_view = unsafe { self.runtime_class_view(bits) };
            if class_view.is_null() {
                return None;
            }
            class_view
        } else {
            unsafe { tag_to_type(tag) }
        };
        let view = if tag == MoltTypeTag::Type {
            let (name, base_bits, semantic_flags) = unsafe { Self::managed_type_metadata(bits) }?;
            let exception_layout = ExceptionLayoutKind::from_u8(unsafe {
                (crate::hooks::hooks_or_stubs().exception_layout_kind)(bits)
            });
            let base = if let Some(base_bits) = base_bits {
                let base_view = unsafe { self.handle_to_borrowed_pyobj(base_bits) };
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(base_bits) };
                if base_view.is_null() {
                    return None;
                }
                base_view.cast::<PyTypeObject>()
            } else {
                &raw mut PyBaseObject_Type
            };
            let inherited_flags = if base.is_null() {
                0
            } else {
                unsafe {
                    (*base).tp_flags
                        & (crate::abi_types::Py_TPFLAGS_LONG_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_LIST_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_TUPLE_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_BYTES_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_UNICODE_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_DICT_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_BASE_EXC_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_TYPE_SUBCLASS)
                }
            };
            let mut object: PyTypeObject = unsafe { std::mem::zeroed() };
            object.ob_base = crate::abi_types::PyVarObject {
                ob_base: PyObject {
                    ob_refcnt,
                    ob_type: &raw mut PyType_Type,
                },
                ob_size: 0,
            };
            object.tp_name = name.as_ptr();
            object.tp_basicsize = if let Some(layout) = exception_layout {
                crate::abi_types::exception_layout_basicsize(layout)
            } else if base.is_null() {
                std::mem::size_of::<PyObject>() as crate::abi_types::Py_ssize_t
            } else {
                unsafe { (*base).tp_basicsize }
            };
            object.tp_itemsize = if exception_layout.is_some() || base.is_null() {
                0
            } else {
                unsafe { (*base).tp_itemsize }
            };
            object.tp_flags = crate::abi_types::Py_TPFLAGS_DEFAULT
                | semantic_flags
                | crate::abi_types::Py_TPFLAGS_READY
                | inherited_flags
                | if exception_layout.is_some() {
                    crate::abi_types::Py_TPFLAGS_BASE_EXC_SUBCLASS
                        | crate::abi_types::Py_TPFLAGS_HAVE_GC
                } else {
                    0
                };
            object.tp_base = base;
            ManagedView::Type {
                object: ManagedTypeAllocation::new(object),
                _name: name,
            }
        } else if tag == MoltTypeTag::Slice {
            ManagedView::Slice(Box::new(UnsafeCell::new(crate::abi_types::PySliceObject {
                ob_base: PyObject { ob_refcnt, ob_type },
                start: std::ptr::null_mut(),
                stop: std::ptr::null_mut(),
                step: std::ptr::null_mut(),
            })))
        } else if tag == MoltTypeTag::MemoryView {
            let mut object: crate::abi_types::PyMemoryViewObject = unsafe { std::mem::zeroed() };
            object.ob_base = PyObject { ob_refcnt, ob_type };
            ManagedView::MemoryView {
                object: Box::new(UnsafeCell::new(object)),
                format: c"B".to_owned(),
            }
        } else if tag == MoltTypeTag::BuiltinCallable {
            let mut object: PyCMethodObject = unsafe { std::mem::zeroed() };
            object.func.ob_base = PyObject { ob_refcnt, ob_type };
            object.func.vectorcall = Some(crate::api::object::molt_runtime_vectorcall);
            ManagedView::RuntimeCallable(Box::new(UnsafeCell::new(object)))
        } else if tag == MoltTypeTag::Tuple {
            let len = unsafe { (crate::hooks::hooks_or_stubs().tuple_len)(bits) };
            let allocation = TupleAllocation::new(ob_refcnt, ob_type, len)?;
            ManagedView::Tuple { allocation }
        } else if tag == MoltTypeTag::List {
            let len = unsafe { (crate::hooks::hooks_or_stubs().list_len)(bits) };
            let allocation = ListAllocation::new(ob_refcnt, ob_type, len)?;
            ManagedView::List { allocation }
        } else if tag == MoltTypeTag::Exception {
            let raw_kind = unsafe { (crate::hooks::hooks_or_stubs().exception_layout_kind)(bits) };
            let layout_kind = ExceptionLayoutKind::from_u8(raw_kind)?;
            if ob_type.is_null()
                || unsafe { (*ob_type).tp_basicsize }
                    != crate::abi_types::exception_layout_basicsize(layout_kind)
            {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"managed exception type/layout size mismatch".as_ptr(),
                    )
                };
                return None;
            }
            ManagedView::Exception(ExceptionAllocation::new(layout_kind, ob_refcnt, ob_type))
        } else {
            ManagedView::Object(Box::new(BridgeHeader {
                py_obj: UnsafeCell::new(PyObject { ob_refcnt, ob_type }),
            }))
        };
        let unicode = if tag == MoltTypeTag::Str {
            let mut len = 0;
            let data = unsafe { (crate::hooks::hooks_or_stubs().str_data)(bits, &mut len) };
            if data.is_null() {
                return None;
            }
            let bytes = unsafe { std::slice::from_raw_parts(data, len) };
            let Some(text) = crate::api::strings::PythonStringBytes::from_bytes(bytes) else {
                unsafe { ensure_result_error(c"invalid internal Python string storage") };
                return None;
            };
            let Some(projection) = UnicodeProjection::from_text(text) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return None;
            };
            Some(projection)
        } else {
            None
        };
        let entry = Box::new(BridgeEntry {
            view,
            bits,
            unicode,
            publication: PublicationState::Building {
                owner: std::thread::current().id(),
            },
            lifecycle: if internal_c_ref {
                BridgeLifecycle::RuntimeOwned
            } else {
                BridgeLifecycle::ViewHoldOnly
            },
        });
        let raw_ptr = entry.view.py_obj();
        Some((entry, raw_ptr))
    }

    unsafe fn published_pyobj(&self, bits: AbiHandle, increment: bool) -> Option<*mut PyObject> {
        let index = self.handle_shard_index(bits);
        let mut handle = self.handle_shards[index].lock();
        loop {
            if let Some(entry) = handle.to_py.get(&bits) {
                match &entry.publication {
                    PublicationState::Ready => {
                        let ptr = entry.view.py_obj();
                        if increment {
                            unsafe { Self::increment_pyobj_ref(ptr) };
                        }
                        return Some(ptr);
                    }
                    PublicationState::Building { owner }
                        if is_publication_owner(self, bits, owner) =>
                    {
                        if !observe_publication(self, bits) {
                            return Some(std::ptr::null_mut());
                        }
                        let ptr = entry.view.py_obj();
                        if increment {
                            unsafe { Self::increment_pyobj_ref(ptr) };
                        }
                        return Some(ptr);
                    }
                    PublicationState::Building { .. } | PublicationState::Retiring => {
                        self.publication_ready[index].wait(&mut handle);
                        continue;
                    }
                }
            }
            if let Some(binding) = handle.raw_py.get(&bits) {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(binding.address);
                if increment {
                    unsafe { Self::increment_pyobj_ref(ptr) };
                }
                return Some(ptr);
            }
            return None;
        }
    }

    /// Release a transaction pin/edge through the same bridge that owns it.
    /// Non-managed edges retain the ordinary native/numeric release authority.
    unsafe fn decref_publication_reference(&self, pointer: *mut PyObject) {
        if pointer.is_null() {
            return;
        }
        match unsafe { self.managed_decref_pyobj(pointer) } {
            ManagedDecref::Immortal | ManagedDecref::Alive | ManagedDecref::RetiredInline => {}
            ManagedDecref::ReleaseRuntimeHold(bits) => unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            },
            ManagedDecref::NotManaged => unsafe {
                crate::api::refcount::Py_DECREF(pointer);
            },
        }
    }

    fn commit_publication_component(&self, first: usize) {
        let end = PUBLICATION_BUILD_STACK.with(|stack| stack.borrow().frames.len());
        // Commit the complete component irreversibly before releasing a pin can
        // run a finalizer. Newly nested views may depend on it from this point.
        PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            for frame in &mut stack.frames[first..end] {
                if frame.committed {
                    continue;
                }
                assert!(frame.complete && !frame.failed && frame.dependency >= first);
                frame.committed = true;
            }
        });
        for index in first..end {
            let candidate = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let frame = &stack.frames[index];
                if frame.pin_released {
                    return None;
                }
                Some((frame.bridge, frame.bits, frame.pointer))
            });
            let Some((bridge, bits, pointer)) = candidate else {
                continue;
            };
            if !pointer.is_null() {
                // Every recorded bridge is borrowed by the still-active outer
                // publication. A cross-bridge child can only remain provisional
                // by depending on that outer component.
                let bridge = unsafe { &*bridge };
                // Building still excludes a concurrent direct retirement. Drop
                // the pin before publishing Ready; an otherwise unowned view
                // may retire here, while every sibling still has its own pin.
                crate::api::errors::with_preserved_error(|| unsafe {
                    bridge.decref_publication_reference(pointer);
                });
                let mut handle = bridge.handle_shard(bits).lock();
                if let Some(entry) = handle.to_py.get_mut(&bits) {
                    assert_eq!(entry.view.py_obj(), pointer);
                    assert!(
                        matches!(&entry.publication, PublicationState::Building { owner } if *owner == std::thread::current().id())
                    );
                    entry.publication = PublicationState::Ready;
                }
            }
            PUBLICATION_BUILD_STACK
                .with(|stack| stack.borrow_mut().frames[index].pin_released = true);
        }
        // Wake waiters after all surviving members have released their pins.
        for index in first..end {
            let (bridge, bits, pointer) = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let frame = &stack.frames[index];
                (frame.bridge, frame.bits, frame.pointer)
            });
            if pointer.is_null() {
                continue;
            }
            let bridge = unsafe { &*bridge };
            bridge.publication_ready[bridge.handle_shard_index(bits)].notify_all();
        }
    }

    /// Roll back every remaining dependent frame. Independent completed views
    /// (including normalized error objects) have already committed and survive.
    /// No allocation is required after failure: the DFS records own retirement.
    fn finish_publication_stack(&self) {
        PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            assert!(stack.active.is_empty());
            stack.rolling_back = true;
        });
        let mut closed = 0;
        let mut drained = 0;
        loop {
            let next = PUBLICATION_BUILD_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                // Close every sibling before the first decref. Callbacks may
                // append frames; each appended frame is closed/drained once.
                while closed < stack.frames.len() {
                    let frame = &mut stack.frames[closed];
                    if !frame.committed {
                        frame.aborting = true;
                    }
                    closed += 1;
                }
                while drained < stack.frames.len() {
                    let frame = &mut stack.frames[drained];
                    drained += 1;
                    if !frame.committed && !frame.edges_cleared {
                        frame.edges_cleared = true;
                        return Some((frame.bridge, frame.bits, frame.pointer));
                    }
                }
                None
            });
            let Some((bridge, bits, pointer)) = next else {
                break;
            };
            if pointer.is_null() {
                continue;
            }
            let bridge = unsafe { &*bridge };
            let entry = {
                let mut handle = bridge.handle_shard(bits).lock();
                let entry = handle
                    .to_py
                    .get_mut(&bits)
                    .expect("rollback lost a pinned view");
                assert_eq!(entry.view.py_obj(), pointer);
                &raw mut **entry
            };
            // The runtime GIL and Building state exclude other users. Keep all
            // entries in both identity maps, with their C pins, until every
            // owned edge has been broken. Internal decrefs therefore still use
            // canonical lifecycle custody and can never free a sibling view.
            unsafe {
                (*entry).view.release_owned_items_with(|edge, mirrored| {
                    if mirrored {
                        bridge.projection_unadopt_owned_ref(edge);
                    }
                    bridge.decref_publication_reference(edge);
                });
            }
        }
        let end = PUBLICATION_BUILD_STACK.with(|stack| stack.borrow().frames.len());
        for index in 0..end {
            let candidate = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let frame = &stack.frames[index];
                (!frame.committed && !frame.pointer.is_null()).then_some((
                    frame.bridge,
                    frame.bits,
                    frame.pointer,
                ))
            });
            let Some((bridge, bits, pointer)) = candidate else {
                continue;
            };
            let bridge = unsafe { &*bridge };
            // Every private incoming owner must be in this retiring component.
            // Check while the allocation and canonical identity are still live;
            // a leftover mirror would become a dangling pointer after removal.
            assert_eq!(
                bridge.mirrored_c_refcount(pointer.addr()),
                0,
                "retiring projection still has an incoming mirrored owner"
            );
            let (mut address, mut handle) = bridge.lock_address_then_handle(pointer.addr(), bits);
            let mut entry = handle
                .to_py
                .remove(&bits)
                .expect("projection retirement identity disappeared");
            assert_eq!(entry.view.py_obj(), pointer);
            entry.publication = PublicationState::Retiring;
            address.from_py.remove(&pointer.addr());
            address.direct_molt_py.remove(&pointer.addr());
            drop(handle);
            drop(address);
            PUBLICATION_BUILD_STACK
                .with(|stack| stack.borrow_mut().frames[index].retired = Some(entry));
        }
        // Clear all old view marks before permitting a retry to publish a new
        // identity. Every old allocation is still alive and already edge-free.
        for index in 0..end {
            let bits = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                stack.frames[index].retired.as_ref().map(|entry| entry.bits)
            });
            if let Some(bits) = bits {
                unsafe {
                    (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 0);
                }
            }
        }
        let mut frames = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            assert!(stack.active.is_empty());
            stack.rolling_back = false;
            std::mem::take(&mut stack.frames)
        });
        // Drop physical allocations only after the whole component is detached.
        for frame in &mut frames {
            drop(frame.retired.take());
        }
        for frame in frames {
            if frame.committed || frame.pointer.is_null() {
                continue;
            }
            let bridge = unsafe { &*frame.bridge };
            bridge.publication_ready[bridge.handle_shard_index(frame.bits)].notify_all();
            unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(frame.bits);
            }
        }
    }

    /// The sole managed-view insertion transaction. A rejected entry is
    /// returned unpublished; the caller unwinds it after both locks drop.
    fn insert_managed_entry(
        &self,
        bits: AbiHandle,
        entry: Box<BridgeEntry>,
    ) -> Result<(), (Box<BridgeEntry>, ManagedEntryRejection)> {
        let addr = entry.view.py_obj().addr();
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        if handle.to_py.contains_key(&bits) || handle.raw_py.contains_key(&bits) {
            return Err((entry, ManagedEntryRejection::Occupied));
        }
        if address.from_py.try_reserve(1).is_err() || handle.to_py.try_reserve(1).is_err() {
            return Err((entry, ManagedEntryRejection::NoMemory));
        }
        if unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 1) } == 0 {
            return Err((entry, ManagedEntryRejection::Deallocating));
        }
        address.from_py.insert(addr, bits);
        handle.to_py.insert(bits, entry);
        Ok(())
    }
}

// Canonical runtime type namespace projection and its physical ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeTypeProjection {
    Managed,
    Native,
}

struct OwnedTypeName(*mut std::os::raw::c_char);
impl OwnedTypeName {
    unsafe fn copy(bytes: *const u8, length: usize) -> Option<Self> {
        let Some(size) = length.checked_add(1) else {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        };
        let pointer =
            unsafe { crate::api::memory::PyMem_Malloc(size) }.cast::<std::os::raw::c_char>();
        if pointer.is_null() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes, pointer.cast(), length);
            *pointer.add(length) = 0;
        }
        Some(Self(pointer))
    }
    fn take(&mut self) -> *mut std::os::raw::c_char {
        std::mem::replace(&mut self.0, std::ptr::null_mut())
    }
}
impl Drop for OwnedTypeName {
    fn drop(&mut self) {
        unsafe { crate::api::memory::PyMem_Free(self.0.cast()) };
    }
}

impl ObjectBridge {
    /// Observe every published class projection without creating a C view.
    /// Noncanonical process bindings have only address-side entries.
    pub fn type_has_projection(&self, bits: AbiHandle) -> bool {
        {
            let handle = self.handle_shard(bits).lock();
            if handle.to_py.contains_key(&bits) || handle.raw_py.contains_key(&bits) {
                return true;
            }
        }
        self.address_shards.iter().any(|shard| {
            shard
                .lock()
                .direct_molt_py
                .values()
                .any(|bound| *bound == bits)
        })
    }

    /// Some(None) is a proven compact managed allocation; it must never fall
    /// through to a foreign or flag-derived storage guess.
    pub(crate) fn managed_type_storage(
        &self,
        pointer: *mut PyTypeObject,
    ) -> Option<Option<*mut crate::abi_types::PyHeapTypeObject>> {
        let bits = self.managed_handle_for_pyobj(pointer.cast())?;
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        if entry.view.py_obj() != pointer.cast() {
            return None;
        }
        Some(match &entry.view {
            ManagedView::Type { object, .. } => object.heap(),
            _ => None,
        })
    }

    fn refresh_type_view(&self, bits: AbiHandle) -> bool {
        let pointer = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return true;
            };
            let ManagedView::Type { object, .. } = &entry.view else {
                return true;
            };
            object.get()
        };
        unsafe { self.publish_type_state(bits, pointer, true) }
    }

    /// Explicit C namespace exposure. Managed Type views mirror a runtime
    /// graph edge; process-owned static shells own a normal C root retired by
    /// retire_static_type_runtime_roots. Neither lane copies dictionary data.
    pub(crate) unsafe fn expose_runtime_type_dictionary(
        &self,
        pointer: *mut PyTypeObject,
    ) -> Result<Option<RuntimeTypeProjection>, ()> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let Some(value) = self.molt_handle_for_pyobj(pointer.cast()) else {
            return Ok(None);
        };
        let bits = value.bits();
        if unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits) }
            != MoltTypeTag::Type as u8
        {
            return Ok(None);
        }
        let mirrored = {
            let handle = self.handle_shard(bits).lock();
            handle.to_py.get(&bits).is_some_and(|entry| {
                entry.view.py_obj() == pointer.cast()
                    && matches!(&entry.view, ManagedView::Type { .. })
            })
        };
        if unsafe { self.publish_type_state(bits, pointer, mirrored) } {
            Ok(Some(if mirrored {
                RuntimeTypeProjection::Managed
            } else {
                RuntimeTypeProjection::Native
            }))
        } else {
            Err(())
        }
    }

    // Address and handle custody must describe this exact retained allocation.
    // Noncanonical static bindings intentionally have no raw_py reverse entry.
    fn type_projection_matches(
        address: &AddressShard,
        handle: &HandleShard,
        bits: AbiHandle,
        pointer: *mut PyTypeObject,
        heap: Option<*mut crate::abi_types::PyHeapTypeObject>,
        mirrored: bool,
    ) -> bool {
        if address.from_py.get(&pointer.addr()) != Some(&bits) {
            return false;
        }
        if mirrored {
            handle.to_py.get(&bits).is_some_and(|entry| {
                entry.view.py_obj() == pointer.cast()
                    && matches!(&entry.view, ManagedView::Type { object, .. } if object.heap() == heap)
            })
        } else {
            address.direct_molt_py.get(&pointer.addr()) == Some(&bits)
                && crate::abi_types::process_heap_type_storage(pointer) == heap
        }
    }

    /// Stage the entire fixed owner inventory before publishing any C field.
    /// Raw process-shell names are allocated before hierarchy/dict publication.
    unsafe fn publish_type_state(
        &self,
        bits: AbiHandle,
        pointer: *mut PyTypeObject,
        mirrored: bool,
    ) -> bool {
        use crate::api::refcount::OwnedPyObject;
        use crate::hooks::TypeMetadataField;
        unsafe {
            let _view_owner = OwnedPyObject::from_borrowed(pointer.cast());
            let heap = if mirrored {
                self.managed_type_storage(pointer).flatten()
            } else {
                crate::abi_types::process_heap_type_storage(pointer)
            };
            let mut values: [OwnedPyObject; 5] =
                std::array::from_fn(|_| OwnedPyObject::from_owned(std::ptr::null_mut()));
            let mut fields = [std::ptr::null_mut(); 5];
            let mut count = 0;
            for (field, destination) in [
                (TypeMetadataField::Bases, &raw mut (*pointer).tp_bases),
                (TypeMetadataField::Mro, &raw mut (*pointer).tp_mro),
            ] {
                values[count] = OwnedPyObject::from_owned(self.owned_result_to_pyobj(
                    (crate::hooks::hooks_or_stubs().type_metadata)(bits, field),
                ));
                if values[count].as_ptr().is_null() {
                    ensure_result_error(c"type metadata projection is missing");
                    return false;
                }
                fields[count] = destination;
                count += 1;
            }
            if let Some(heap) = heap {
                for (field, destination) in [
                    (TypeMetadataField::Name, &raw mut (*heap).ht_name),
                    (TypeMetadataField::QualName, &raw mut (*heap).ht_qualname),
                ] {
                    values[count] = OwnedPyObject::from_owned(self.owned_result_to_pyobj(
                        (crate::hooks::hooks_or_stubs().type_metadata)(bits, field),
                    ));
                    if values[count].as_ptr().is_null() {
                        ensure_result_error(c"type metadata projection is missing");
                        return false;
                    }
                    fields[count] = destination;
                    count += 1;
                }
            }
            let dictionary =
                self.borrowed_result_to_borrowed_pyobj((crate::hooks::hooks_or_stubs()
                    .type_dict_borrowed)(
                    bits
                ));
            if dictionary.is_null() {
                ensure_result_error(c"type dictionary projection is missing");
                return false;
            }
            values[count] = OwnedPyObject::from_borrowed(dictionary);
            fields[count] = &raw mut (*pointer).tp_dict;
            count += 1;
            let mut shell_name = OwnedTypeName(std::ptr::null_mut());
            if !mirrored
                && let Some(heap) = heap
                && (*heap)._ht_tpname.is_null()
            {
                let mut length = 0;
                let bytes = crate::api::strings::PyUnicode_AsUTF8AndSize(
                    values[2].as_ptr(),
                    &raw mut length,
                );
                if bytes.is_null() {
                    return false;
                }
                let Some(prepared) = OwnedTypeName::copy(bytes.cast(), length as usize) else {
                    return false;
                };
                shell_name = prepared;
            }
            let mut acquired = [std::ptr::null_mut(); 5];
            for index in 0..count {
                let value = values[index].as_ptr();
                if mirrored {
                    if !self.projection_incref(value) {
                        crate::api::errors::with_preserved_error(|| {
                            for root in acquired {
                                self.projection_decref(root);
                            }
                        });
                        return false;
                    }
                } else {
                    crate::api::refcount::Py_INCREF(value);
                }
                acquired[index] = value;
            }
            let mut previous = [std::ptr::null_mut(); 5];
            let committed = {
                let (address, handle) = self.lock_address_then_handle(pointer.addr(), bits);
                let same =
                    Self::type_projection_matches(&address, &handle, bits, pointer, heap, mirrored);
                if same {
                    for index in 0..count {
                        previous[index] = fields[index].replace(acquired[index]);
                    }
                    if let Some(heap) = heap
                        && !shell_name.0.is_null()
                    {
                        (*heap)._ht_tpname = shell_name.take();
                        (*pointer).tp_name = (*heap)._ht_tpname;
                    }
                }
                same
            };
            if !committed {
                ensure_result_error(c"type projection identity changed before publication");
            }
            crate::api::errors::with_preserved_error(|| {
                for root in if committed { previous } else { acquired } {
                    if mirrored {
                        self.projection_decref(root);
                    } else {
                        crate::api::refcount::Py_XDECREF(root);
                    }
                }
            });
            committed
        }
    }
}

// Canonical Molt-handle to CPython-view publication and result ownership.
impl ObjectBridge {
    unsafe fn handle_to_pyobj_impl(&self, bits: AbiHandle, owned: bool) -> *mut PyObject {
        // Pin before any shard lock.  Runtime ownership, physical projection
        // population, and publication state form one bridge transaction.
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        if let Some(ptr) = Self::singleton_pyobj(bits) {
            return ptr;
        }
        if let Some(ptr) = unsafe { self.published_pyobj(bits, owned) } {
            if owned {
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
            }
            if ptr.is_null() {
                unsafe {
                    ensure_result_error(c"ABI publication is rolling back");
                }
            }
            return ptr;
        }

        // A cold physical view may materialize fields or a type namespace.
        // Incoming errors belong to the caller, not to those result hooks.
        // Preserve them across the complete publication and rollback, without
        // re-entering this decision through a possibly partial bootstrap hook.
        let publish = || unsafe { self.publish_cold_pyobj(bits, owned) };
        if crate::api::errors::raised_error_pending() {
            crate::api::errors::with_preserved_error(publish)
        } else {
            publish()
        }
    }

    /// Publish under the caller's runtime GIL and error-preservation boundary.
    unsafe fn publish_cold_pyobj(&self, bits: AbiHandle, owned: bool) -> *mut PyObject {
        let Some(build_guard) = PublicationBuildGuard::enter(self, bits) else {
            if owned {
                crate::api::errors::with_preserved_error(|| unsafe {
                    (crate::hooks::hooks_or_stubs().dec_ref)(bits);
                });
            }
            return std::ptr::null_mut();
        };
        loop {
            // A borrowed crossing must manufacture the stable runtime hold;
            // an owned crossing transfers its incoming hold to the view.
            if !owned {
                unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(bits) };
            }
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
            let (initial_refs, has_non_view_runtime_owner) =
                initial_managed_view_refs(runtime_refs, owned);
            let pinned_refs = if crate::abi_types::is_immortal_refcnt(initial_refs) {
                initial_refs
            } else {
                initial_refs + 1
            };
            let Some((entry, raw_ptr)) =
                // One temporary C pin keeps even an otherwise unowned nested
                // view alive until its recursive component commits or unwinds.
                (unsafe { self.build_pyobj_entry(bits, pinned_refs, has_non_view_runtime_owner) })
            else {
                crate::api::errors::with_preserved_error(|| unsafe {
                    (crate::hooks::hooks_or_stubs().dec_ref)(bits)
                });
                drop(build_guard);
                unsafe {
                    ensure_result_error(c"managed runtime handle could not build an ABI view")
                };
                return std::ptr::null_mut();
            };
            match self.insert_managed_entry(bits, entry) {
                Ok(()) => build_guard.inserted(raw_ptr),
                Err((_, ManagedEntryRejection::Occupied)) => {
                    if !owned {
                        unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                    }
                    if let Some(ptr) = unsafe { self.published_pyobj(bits, owned) } {
                        if owned {
                            unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                        }
                        if !build_guard.finish() {
                            return std::ptr::null_mut();
                        }
                        return ptr;
                    }
                    continue;
                }
                Err((_, rejection)) => {
                    crate::api::errors::with_preserved_error(|| unsafe {
                        (crate::hooks::hooks_or_stubs().dec_ref)(bits);
                    });
                    drop(build_guard);
                    unsafe { rejection.set_error() };
                    return std::ptr::null_mut();
                }
            }

            if !self.refresh_type_view(bits)
                || !self.refresh_tuple_view(bits)
                || !self.refresh_slice_view_with_origins(bits, None)
                || !self.refresh_list_view(bits)
                || !self.refresh_exception_view(bits)
                || !self.refresh_memoryview_view(bits)
            {
                drop(build_guard);
                unsafe {
                    ensure_result_error(c"managed runtime handle could not populate an ABI view")
                };
                return std::ptr::null_mut();
            }
            if !build_guard.finish() {
                unsafe {
                    ensure_result_error(c"recursive ABI view publication failed");
                }
                return std::ptr::null_mut();
            }
            return raw_ptr;
        }
    }

    /// Translate a Molt handle to a new-reference `PyObject*`.
    pub unsafe fn owned_handle_to_pyobj(&self, bits: AbiHandle) -> *mut PyObject {
        unsafe { self.handle_to_pyobj_impl(bits, true) }
    }

    /// Translate a Molt handle to a borrowed `PyObject*`.
    pub unsafe fn handle_to_borrowed_pyobj(&self, bits: AbiHandle) -> *mut PyObject {
        unsafe { self.handle_to_pyobj_impl(bits, false) }
    }

    /// Acquire a new C reference without consuming the caller's runtime owner.
    /// All borrowed runtime arguments use this crossing, including foreign
    /// slot names, values, positional tuples, and keyword dictionaries.
    pub unsafe fn borrowed_handle_to_new_pyobj(&self, bits: AbiHandle) -> *mut PyObject {
        unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(bits) };
        unsafe { self.owned_handle_to_pyobj(bits) }
    }

    pub unsafe fn owned_result_to_pyobj(
        &self,
        result: crate::hooks::OwnedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::raised_error_pending() {
                    crate::api::errors::with_preserved_error(|| unsafe {
                        (crate::hooks::hooks_or_stubs().dec_ref)(bits)
                    });
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime owned-result hook returned a value with an exception set",
                        )
                    };
                    std::ptr::null_mut()
                } else {
                    unsafe { self.owned_handle_to_pyobj(bits) }
                }
            }
            crate::hooks::DecodedHandleResult::Missing => {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                std::ptr::null_mut()
            }
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    ensure_result_error(c"runtime owned-result hook failed without an exception")
                };
                std::ptr::null_mut()
            }
        }
    }
}

// Borrowed/new-result projection ownership at the C ABI boundary.
impl ObjectBridge {
    pub unsafe fn borrowed_result_to_borrowed_pyobj(
        &self,
        result: crate::hooks::BorrowedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::raised_error_pending() {
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime borrowed-result hook returned a value with an exception set",
                        )
                    };
                    std::ptr::null_mut()
                } else {
                    let ptr = unsafe { self.handle_to_borrowed_pyobj(bits) };
                    if ptr.is_null() {
                        unsafe {
                            ensure_result_error(
                                c"runtime borrowed-result handle could not enter the bridge",
                            )
                        };
                    }
                    ptr
                }
            }
            crate::hooks::DecodedHandleResult::Missing => {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                std::ptr::null_mut()
            }
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    ensure_result_error(c"runtime borrowed-result hook failed without an exception")
                };
                std::ptr::null_mut()
            }
        }
    }

    pub unsafe fn borrowed_result_to_new_pyobj(
        &self,
        result: crate::hooks::BorrowedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::raised_error_pending() {
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime borrowed-result hook returned a value with an exception set",
                        )
                    };
                    return std::ptr::null_mut();
                }
                let ptr = unsafe { self.borrowed_handle_to_new_pyobj(bits) };
                if ptr.is_null() {
                    unsafe {
                        ensure_result_error(
                            c"runtime borrowed-result handle could not enter the bridge",
                        )
                    };
                    return std::ptr::null_mut();
                }
                ptr
            }
            crate::hooks::DecodedHandleResult::Missing => {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                std::ptr::null_mut()
            }
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    ensure_result_error(c"runtime borrowed-result hook failed without an exception")
                };
                std::ptr::null_mut()
            }
        }
    }

    #[inline(always)]
    pub fn pyobj_to_handle(&self, ptr: *mut PyObject) -> Option<BridgeIdentity> {
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            return Some(BridgeIdentity(bits));
        }
        let addr = ptr.addr();
        loop {
            let address = self.address_shard(addr).lock();
            if let Some(bits) = address
                .numeric_carriers
                .get(&addr)
                .and_then(|record| record.bits)
            {
                return Some(BridgeIdentity(bits));
            }
            let bits = address.from_py.get(&addr).copied()?;
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let visible = handle
                .raw_py
                .get(&bits)
                .is_some_and(|binding| binding.address == addr)
                || handle.to_py.get(&bits).is_some_and(|entry| {
                    entry.view.py_obj().addr() == addr
                        && (matches!(entry.publication, PublicationState::Ready)
                            || matches!(
                                &entry.publication,
                                PublicationState::Building { owner }
                                    if is_publication_owner(self, bits, owner)
                            ))
                });
            if visible {
                return Some(BridgeIdentity(bits));
            }
            let waiting = handle.to_py.get(&bits).is_some_and(|entry| {
                matches!(
                    entry.publication,
                    PublicationState::Building { .. } | PublicationState::Retiring
                )
            });
            drop(address);
            if !waiting {
                return None;
            }
            self.publication_ready[index].wait(&mut handle);
        }
    }
}

// Physical sequence projection preparation and publication.
impl ObjectBridge {
    /// Populate once before publication. Runtime geometry and format are immutable;
    /// explicit release only invalidates the projection, so borrowed C pointers
    /// remain stable across GET_BUFFER calls and unrelated view allocations.
    pub fn refresh_memoryview_view(&self, bits: AbiHandle) -> bool {
        {
            let handle = self.handle_shard(bits).lock();
            if !matches!(
                handle.to_py.get(&bits).map(|entry| &entry.view),
                Some(ManagedView::MemoryView { .. })
            ) {
                return true;
            }
        }
        let mut descriptor = crate::hooks::MoltBufferView::default();
        let mut native_base = std::ptr::null_mut();
        let mut format_bytes = std::ptr::null();
        let mut format_len = 0;
        let status = unsafe {
            (crate::hooks::hooks_or_stubs().memoryview_snapshot)(
                bits,
                &mut descriptor,
                &mut native_base,
                &mut format_bytes,
                &mut format_len,
            )
        };
        if status < 0 {
            return false;
        }
        if status == 1 {
            self.invalidate_memoryview_view(bits);
            return true;
        }
        if format_bytes.is_null() || descriptor.ndim as usize > crate::hooks::MOLT_BUFFER_MAX_NDIM {
            return false;
        }
        let Ok(format) =
            std::ffi::CString::new(unsafe { std::slice::from_raw_parts(format_bytes, format_len) })
        else {
            return false;
        };
        let base = if !native_base.is_null() {
            native_base
        } else if descriptor.base != 0 {
            unsafe { self.handle_to_borrowed_pyobj(descriptor.base) }
        } else {
            std::ptr::null_mut()
        };
        if descriptor.base != 0 && base.is_null() {
            return false;
        }
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return false;
        };
        let ManagedView::MemoryView {
            object,
            format: stored_format,
        } = &mut entry.view
        else {
            return false;
        };
        *stored_format = format;
        unsafe {
            let object = object.get();
            (*object).base = base;
            (*object).ob_shape = descriptor.shape;
            (*object).ob_strides = descriptor.strides;
            let view = &raw mut (*object).view;
            (*view).buf = descriptor.data.cast();
            (*view).obj = base;
            (*view).len = descriptor.len as isize;
            (*view).itemsize = descriptor.itemsize as isize;
            (*view).readonly = descriptor.readonly as i32;
            (*view).ndim = descriptor.ndim as i32;
            (*view).format = stored_format.as_ptr().cast_mut();
            (*view).shape = (&raw mut (*object).ob_shape).cast();
            (*view).strides = (&raw mut (*object).ob_strides).cast();
            (*view).suboffsets = std::ptr::null_mut();
            (*view).internal = std::ptr::null_mut();
        }
        true
    }

    /// Runtime ownership is already empty before any exporter release callback.
    pub fn invalidate_memoryview_view(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        if let ManagedView::MemoryView { object, .. } = &mut entry.view {
            unsafe {
                (*object.get()).base = std::ptr::null_mut();
                (&raw mut (*object.get()).view).write(std::mem::zeroed());
            }
        }
    }

    /// Publish an exact name/qualname projection before the runtime slot commits.
    /// Handles are borrowed from the mutation's existing owner. Projection
    /// creation runs outside map locks; release follows publication of every C
    /// field. Static bound heap shells share their existing normal-C ownership.
    pub fn update_type_identity_view(
        &self,
        bits: AbiHandle,
        value: AbiHandle,
        qualname: bool,
    ) -> bool {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let (pointer, heap, mirrored) = {
            let handle = self.handle_shard(bits).lock();
            if let Some(entry) = handle.to_py.get(&bits) {
                let ManagedView::Type { object, .. } = &entry.view else {
                    return true;
                };
                (object.get(), object.heap(), true)
            } else if let Some(raw) = handle.raw_py.get(&bits) {
                let pointer = std::ptr::with_exposed_provenance_mut::<PyTypeObject>(raw.address);
                (
                    pointer,
                    crate::abi_types::process_heap_type_storage(pointer),
                    false,
                )
            } else {
                return true;
            }
        };
        unsafe {
            let _view_owner = crate::api::refcount::OwnedPyObject::from_borrowed(pointer.cast());
            let Some(heap) = heap else {
                ensure_result_error(c"compact type identity cannot change");
                return false;
            };
            let mut name = None;
            let mut shell_name = OwnedTypeName(std::ptr::null_mut());
            if !qualname {
                let mut length = 0;
                let bytes = (crate::hooks::hooks_or_stubs().str_data)(value, &raw mut length);
                if bytes.is_null() {
                    ensure_result_error(c"type name source is missing");
                    return false;
                }
                let source = std::slice::from_raw_parts(bytes, length);
                if source.contains(&0) {
                    ensure_result_error(c"invalid managed type name");
                    return false;
                }
                if mirrored {
                    let Some(size) = length.checked_add(1) else {
                        crate::api::errors::PyErr_NoMemory();
                        return false;
                    };
                    let mut owned = Vec::new();
                    if owned.try_reserve_exact(size).is_err() {
                        crate::api::errors::PyErr_NoMemory();
                        return false;
                    }
                    owned.extend_from_slice(source);
                    owned.push(0);
                    name = Some(std::ffi::CString::from_vec_with_nul_unchecked(owned));
                } else {
                    let Some(prepared) = OwnedTypeName::copy(bytes, length) else {
                        return false;
                    };
                    shell_name = prepared;
                }
            }
            let projected = self.handle_to_borrowed_pyobj(value);
            if projected.is_null() {
                ensure_result_error(c"type name projection is missing");
                return false;
            }
            if mirrored {
                if !self.projection_incref(projected) {
                    return false;
                }
            } else {
                crate::api::refcount::Py_INCREF(projected);
            }
            let mut retired_name = std::ptr::null_mut();
            let mut retired_cstring = None;
            let mut old = std::ptr::null_mut();
            let committed = {
                let (address, mut handle) = self.lock_address_then_handle(pointer.addr(), bits);
                let same = Self::type_projection_matches(
                    &address,
                    &handle,
                    bits,
                    pointer,
                    Some(heap),
                    mirrored,
                );
                if same {
                    if let Some(name) = name {
                        let Some(entry) = handle.to_py.get_mut(&bits) else {
                            unreachable!()
                        };
                        let ManagedView::Type { _name, .. } = &mut entry.view else {
                            unreachable!()
                        };
                        (*pointer).tp_name = name.as_ptr();
                        retired_cstring = Some(std::mem::replace(_name, name));
                    } else if !shell_name.0.is_null() {
                        retired_name = (&raw mut (*heap)._ht_tpname).replace(shell_name.take());
                        (*pointer).tp_name = (*heap)._ht_tpname;
                    }
                    old = (if qualname {
                        &raw mut (*heap).ht_qualname
                    } else {
                        &raw mut (*heap).ht_name
                    })
                    .replace(projected);
                }
                same
            };
            if !committed {
                ensure_result_error(c"type identity changed before name publication");
            }
            crate::api::errors::with_preserved_error(|| {
                crate::api::memory::PyMem_Free(retired_name.cast());
                drop(retired_cstring);
                let release = if committed { old } else { projected };
                if mirrored {
                    self.projection_decref(release);
                } else {
                    crate::api::refcount::Py_XDECREF(release);
                }
            });
            committed
        }
    }

    /// Populate the packed tuple projection from the sole runtime tuple
    /// authority before the pointer is published. Every non-NULL physical slot
    /// owns one projection-ledger C edge. Open tuples legitimately expose NULL
    /// construction slots; after publication they change only through the
    /// exact fixed-slot `PreparedTupleValue` transaction.
    pub fn refresh_tuple_view(&self, bits: AbiHandle) -> bool {
        let is_tuple = {
            let handle = self.handle_shard(bits).lock();
            matches!(
                handle.to_py.get(&bits).map(|entry| &entry.view),
                Some(ManagedView::Tuple { .. })
            )
        };
        if !is_tuple {
            return true;
        }
        let hooks = crate::hooks::hooks_or_stubs();
        let len = unsafe { (hooks.tuple_len)(bits) };
        let mut staged: Vec<*mut PyObject> = Vec::new();
        if staged.try_reserve_exact(len).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        }
        for index in 0..len {
            let result = unsafe { (hooks.tuple_item)(bits, index) };
            let pointer = match result.decode() {
                crate::hooks::DecodedHandleResult::Ok(item_bits) => {
                    let Some(pointer) = self.list_projection_pointer(item_bits) else {
                        for pointer in staged {
                            unsafe { self.projection_decref(pointer) };
                        }
                        return false;
                    };
                    pointer
                }
                crate::hooks::DecodedHandleResult::Missing => std::ptr::null_mut(),
                crate::hooks::DecodedHandleResult::Error => {
                    for pointer in staged {
                        unsafe { self.projection_decref(pointer) };
                    }
                    return false;
                }
            };
            staged.push(pointer);
        }
        let mut handle = self.handle_shard(bits).lock();
        let valid_tuple_view = matches!(
            handle.to_py.get(&bits).map(|entry| &entry.view),
            Some(ManagedView::Tuple { .. })
        );
        if !valid_tuple_view {
            drop(handle);
            for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
                unsafe { self.projection_decref(pointer) };
            }
            return false;
        }
        let entry = handle
            .to_py
            .get_mut(&bits)
            .expect("tuple view disappeared while its handle shard was locked");
        let ManagedView::Tuple { allocation } = &mut entry.view else {
            unreachable!("tuple view changed kind while its handle shard was locked")
        };
        if allocation.len != len {
            drop(handle);
            for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
                unsafe { self.projection_decref(pointer) };
            }
            return false;
        }
        for (index, staged_item) in staged.iter_mut().enumerate() {
            std::mem::swap(&mut allocation.items_mut()[index], staged_item);
        }
        drop(handle);
        for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
            unsafe { self.projection_decref(pointer) };
        }
        true
    }

    /// Adopt the reference stolen by `PyTuple_SetItem` before runtime mutation.
    /// Failure consumes that stolen reference exactly as CPython requires.
    pub unsafe fn prepare_tuple_value(
        &self,
        bits: AbiHandle,
        index: usize,
        value_bits: AbiHandle,
        pointer: *mut PyObject,
    ) -> Option<PreparedTupleValue> {
        let valid_slot = {
            let handle = self.handle_shard(bits).lock();
            matches!(
                handle.to_py.get(&bits).map(|entry| &entry.view),
                Some(ManagedView::Tuple { allocation }) if index < allocation.len
            )
        };
        if !valid_slot || (!pointer.is_null() && !self.pyobj_matches_handle(pointer, value_bits)) {
            unsafe { crate::api::refcount::Py_XDECREF(pointer) };
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"PyTuple_SetItem projection identity mismatch".as_ptr(),
                    )
                };
            }
            return None;
        }
        if !unsafe { self.projection_adopt_owned_ref(pointer) } {
            unsafe {
                crate::api::refcount::Py_DECREF(pointer);
                crate::api::errors::PyErr_NoMemory();
            }
            return None;
        }
        Some(PreparedTupleValue {
            bits,
            index,
            pointer: Some(pointer),
        })
    }

    pub fn tuple_view_item_pointer(&self, bits: AbiHandle, index: usize) -> Option<*mut PyObject> {
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        let ManagedView::Tuple { allocation } = &entry.view else {
            return None;
        };
        allocation.items().get(index).copied()
    }

    fn list_projection_pointer(&self, item_bits: AbiHandle) -> Option<*mut PyObject> {
        if crate::api::numbers::is_numeric_handle(item_bits) {
            let (pointer, already_owned) =
                unsafe { crate::api::numbers::materialize_numeric_borrowed_handle(item_bits) };
            if pointer.is_null() {
                return None;
            }
            if already_owned {
                if !unsafe { self.projection_adopt_owned_ref(pointer) } {
                    unsafe { crate::api::refcount::Py_DECREF(pointer) };
                    return None;
                }
            } else if !unsafe { self.projection_incref(pointer) } {
                return None;
            }
            return Some(pointer);
        }
        let pointer = unsafe { self.handle_to_borrowed_pyobj(item_bits) };
        if pointer.is_null() {
            return None;
        }
        if !unsafe { self.projection_incref(pointer) } {
            return None;
        }
        Some(pointer)
    }

    fn prepare_list_value(
        &self,
        bits: AbiHandle,
        item_bits: AbiHandle,
        item_ptr: *mut PyObject,
        reserve_insert: bool,
    ) -> Option<PreparedListValue> {
        let expected_len = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            let ManagedView::List { allocation } = &entry.view else {
                return None;
            };
            if !allocation.sealed || allocation.items != allocation.shadow {
                drop(handle);
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"cannot mutate a dirty or incomplete list projection".as_ptr(),
                    )
                };
                return None;
            }
            allocation.items.len()
        };
        let pointer = if item_ptr.is_null() {
            self.list_projection_pointer(item_bits)?
        } else {
            if !self.pyobj_matches_handle(item_ptr, item_bits) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"list mutation origin pointer does not match its runtime handle".as_ptr(),
                    )
                };
                return None;
            }
            if !unsafe { self.projection_incref(item_ptr) } {
                return None;
            }
            item_ptr
        };
        if reserve_insert {
            let reserved = {
                let mut handle = self.handle_shard(bits).lock();
                match handle.to_py.get_mut(&bits) {
                    Some(entry) => match &mut entry.view {
                        ManagedView::List { allocation }
                            if allocation.sealed
                                && allocation.items == allocation.shadow
                                && allocation.items.len() == expected_len =>
                        {
                            let items = allocation.items.try_reserve(1).is_ok();
                            // `items` may have moved even if a later reserve
                            // fails, so keep the C-visible pointer truthful.
                            let shadow = allocation.shadow.try_reserve(1).is_ok();
                            let initialized = allocation.initialized.try_reserve(1).is_ok();
                            allocation.publish_storage();
                            items && shadow && initialized
                        }
                        _ => false,
                    },
                    None => false,
                }
            };
            if !reserved {
                unsafe {
                    self.projection_decref(pointer);
                    crate::api::errors::PyErr_NoMemory();
                }
                return None;
            }
        }
        Some(PreparedListValue {
            bits,
            expected_len,
            pointer: Some(pointer),
        })
    }
}

// Prepared list insert/set/projection transactions.
impl ObjectBridge {
    /// Stage one physical projection edge plus capacity for an insertion.
    pub fn prepare_list_insert(
        &self,
        bits: AbiHandle,
        item_bits: AbiHandle,
    ) -> Option<PreparedListValue> {
        self.prepare_list_value(bits, item_bits, std::ptr::null_mut(), true)
    }

    /// Stage an insertion from the exact originating C object. This preserves
    /// CPython identity and reuses the existing carrier instead of allocating
    /// an equivalent scalar proxy.
    pub fn prepare_list_insert_from_pyobj(
        &self,
        bits: AbiHandle,
        item_bits: AbiHandle,
        item_ptr: *mut PyObject,
    ) -> Option<PreparedListValue> {
        self.prepare_list_value(bits, item_bits, item_ptr, true)
    }

    /// Stage one physical projection edge for an indexed replacement.
    pub fn prepare_list_set(
        &self,
        bits: AbiHandle,
        item_bits: AbiHandle,
    ) -> Option<PreparedListValue> {
        self.prepare_list_value(bits, item_bits, std::ptr::null_mut(), false)
    }

    pub fn prepare_list_set_from_pyobj(
        &self,
        bits: AbiHandle,
        item_bits: AbiHandle,
        item_ptr: *mut PyObject,
    ) -> Option<PreparedListValue> {
        self.prepare_list_value(bits, item_bits, item_ptr, false)
    }

    /// Stage a complete future list projection without mutating the live view.
    /// This is the batch path for splice/reorder transactions; common indexed
    /// and append mutations use delta publication instead.
    pub fn prepare_list_projection(
        &self,
        bits: AbiHandle,
        handles: &[AbiHandle],
    ) -> Option<PreparedListProjection> {
        self.prepare_list_projection_inner(bits, handles, None)
    }

    /// Stage a complete future projection from exact physical objects. This is
    /// the batch counterpart of exact-origin append/insert and is the sole path
    /// for slice/extend/repeat transactions that must preserve C identity.
    pub fn prepare_list_projection_from_pyobjs(
        &self,
        bits: AbiHandle,
        handles: &[AbiHandle],
        pointers: &[*mut PyObject],
    ) -> Option<PreparedListProjection> {
        if handles.len() != pointers.len() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"list projection handle/pointer length mismatch".as_ptr(),
                )
            };
            return None;
        }
        self.prepare_list_projection_inner(bits, handles, Some(pointers))
    }

    fn prepare_list_projection_inner(
        &self,
        bits: AbiHandle,
        handles: &[AbiHandle],
        exact_pointers: Option<&[*mut PyObject]>,
    ) -> Option<PreparedListProjection> {
        {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            let ManagedView::List { allocation } = &entry.view else {
                return None;
            };
            if !allocation.sealed || allocation.items != allocation.shadow {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"cannot stage a runtime mutation for a dirty list projection".as_ptr(),
                    )
                };
                return None;
            }
        }
        let mut items: Vec<*mut PyObject> = Vec::new();
        let mut shadow: Vec<*mut PyObject> = Vec::new();
        let mut initialized: Vec<bool> = Vec::new();
        if items.try_reserve_exact(handles.len()).is_err()
            || shadow.try_reserve_exact(handles.len()).is_err()
            || initialized.try_reserve_exact(handles.len()).is_err()
        {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        for (index, &item_bits) in handles.iter().enumerate() {
            let pointer = if let Some(pointers) = exact_pointers {
                let pointer = pointers[index];
                if !self.pyobj_matches_handle(pointer, item_bits)
                    || !unsafe { self.projection_incref(pointer) }
                {
                    None
                } else {
                    Some(pointer)
                }
            } else {
                self.list_projection_pointer(item_bits)
            };
            let Some(pointer) = pointer else {
                for pointer in items.into_iter().filter(|pointer| !pointer.is_null()) {
                    unsafe { self.projection_decref(pointer) };
                }
                return None;
            };
            items.push(pointer);
        }
        shadow.extend_from_slice(&items);
        initialized.resize(handles.len(), true);
        Some(PreparedListProjection {
            bits,
            items: Some(items),
            shadow: Some(shadow),
            initialized: Some(initialized),
        })
    }

    fn publish_prepared_list_projection(
        &self,
        prepared: &mut PreparedListProjection,
    ) -> RetiredListProjection {
        let next_items = prepared
            .items
            .take()
            .expect("prepared list projection published twice");
        let next_shadow = prepared
            .shadow
            .take()
            .expect("prepared list projection shadow missing");
        let next_initialized = prepared
            .initialized
            .take()
            .expect("prepared list projection initialization missing");
        let old = {
            let mut handle = self.handle_shard(prepared.bits).lock();
            let Some(entry) = handle.to_py.get_mut(&prepared.bits) else {
                eprintln!("molt fatal: list view disappeared during prepared publication");
                std::process::abort();
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                eprintln!("molt fatal: list view changed kind during prepared publication");
                std::process::abort();
            };
            if !allocation.sealed || allocation.items != allocation.shadow {
                eprintln!("molt fatal: list projection became dirty during runtime mutation");
                std::process::abort();
            }
            let old = std::mem::take(&mut allocation.shadow);
            allocation.items = next_items;
            allocation.shadow = next_shadow;
            allocation.initialized = next_initialized;
            allocation.uninitialized_count = 0;
            allocation.sealed = true;
            allocation.publish_storage();
            old
        };
        RetiredListProjection { pointers: old }
    }
}

// In-place list projection mutations that reuse already-prepared storage.
impl ObjectBridge {
    /// Move a clean complete physical projection off the live list and publish
    /// an empty PyListObject without changing projection reference counts.
    /// The returned projection can be reordered and restored after arbitrary
    /// sort callbacks without any fallible allocation.
    pub fn detach_list_projection_for_sort(
        &self,
        bits: AbiHandle,
    ) -> Option<PreparedListProjection> {
        let mut handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get_mut(&bits)?;
        let ManagedView::List { allocation } = &mut entry.view else {
            return None;
        };
        if !allocation.sealed || allocation.items != allocation.shadow {
            drop(handle);
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"cannot sort a dirty or incomplete list projection".as_ptr(),
                )
            };
            return None;
        }
        let items = std::mem::take(&mut allocation.items);
        let shadow = std::mem::take(&mut allocation.shadow);
        let initialized = std::mem::take(&mut allocation.initialized);
        allocation.sealed = true;
        allocation.publish_storage();
        Some(PreparedListProjection {
            bits,
            items: Some(items),
            shadow: Some(shadow),
            initialized: Some(initialized),
        })
    }

    /// Publish an in-place runtime swap into a clean physical list projection.
    /// Reordering retains the same projection references, so this is O(1),
    /// allocation-free, and performs no refcount traffic.
    pub fn publish_list_swap(&self, bits: AbiHandle, left: usize, right: usize) -> bool {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return false;
        };
        let ManagedView::List { allocation } = &mut entry.view else {
            return false;
        };
        if !allocation.sealed
            || allocation.items != allocation.shadow
            || left >= allocation.items.len()
            || right >= allocation.items.len()
        {
            drop(handle);
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"cannot reorder a dirty, incomplete, or invalid list projection".as_ptr(),
                )
            };
            return false;
        }
        allocation.items.swap(left, right);
        allocation.shadow.swap(left, right);
        allocation.initialized.swap(left, right);
        allocation.publish_storage();
        true
    }

    /// Publish one runtime removal without allocation. The displaced physical
    /// edge is returned for release after the runtime generation is visible.
    pub fn publish_list_remove(&self, bits: AbiHandle, index: usize) -> Option<RetiredListItem> {
        let pointer = {
            let mut handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get_mut(&bits)?;
            let ManagedView::List { allocation } = &mut entry.view else {
                return None;
            };
            if !allocation.sealed
                || allocation.items != allocation.shadow
                || index >= allocation.items.len()
            {
                return None;
            }
            allocation.items.remove(index);
            let pointer = allocation.shadow.remove(index);
            let was_initialized = allocation.initialized.remove(index);
            if !was_initialized {
                allocation.uninitialized_count = allocation.uninitialized_count.saturating_sub(1);
            }
            allocation.publish_storage();
            pointer
        };
        Some(RetiredListItem {
            pointer: Some(pointer),
        })
    }

    /// Publish a complete-list reversal with no allocation or refcount traffic.
    pub fn publish_list_reverse(&self, bits: AbiHandle) -> bool {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return false;
        };
        let ManagedView::List { allocation } = &mut entry.view else {
            return false;
        };
        if !allocation.sealed || allocation.items != allocation.shadow {
            return false;
        }
        allocation.items.reverse();
        allocation.shadow.reverse();
        allocation.initialized.reverse();
        true
    }

    /// Publish an empty physical list and retire all projection edges without
    /// allocating a replacement pointer array.
    pub fn publish_list_clear(&self, bits: AbiHandle) -> Option<RetiredListProjection> {
        let pointers = {
            let mut handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get_mut(&bits)?;
            let ManagedView::List { allocation } = &mut entry.view else {
                return None;
            };
            if !allocation.sealed || allocation.items != allocation.shadow {
                return None;
            }
            allocation.items.clear();
            let pointers = std::mem::take(&mut allocation.shadow);
            allocation.initialized.clear();
            allocation.uninitialized_count = 0;
            allocation.sealed = true;
            allocation.publish_storage();
            pointers
        };
        Some(RetiredListProjection { pointers })
    }

    /// Pull the canonical runtime list into its CPython pointer-array
    /// projection. Every published item owns one separately ledgered C edge;
    /// old edges are released only after the new object header and buffer have
    /// been published under the handle shard.
    pub fn refresh_list_view(&self, bits: AbiHandle) -> bool {
        let (old_len, initialized) = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return true;
            };
            let ManagedView::List { allocation } = &entry.view else {
                return true;
            };
            (allocation.items.len(), allocation.initialized.clone())
        };
        let hooks = crate::hooks::hooks_or_stubs();
        let len = unsafe { (hooks.list_len)(bits) };
        let preserve_uninitialized = len == old_len && initialized.iter().any(|value| !*value);
        let next_initialized = if preserve_uninitialized {
            initialized
        } else {
            vec![true; len]
        };
        let mut staged: Vec<*mut PyObject> = Vec::new();
        if staged.try_reserve_exact(len).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        }
        for (index, is_initialized) in next_initialized.iter().copied().enumerate() {
            if !is_initialized {
                staged.push(std::ptr::null_mut());
                continue;
            }
            let result = unsafe { (hooks.list_item)(bits, index) };
            let crate::hooks::DecodedHandleResult::Ok(item_bits) = result.decode() else {
                for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
                    unsafe { self.projection_decref(pointer) };
                }
                unsafe { ensure_result_error(c"runtime list snapshot item missing") };
                return false;
            };
            let Some(pointer) = self.list_projection_pointer(item_bits) else {
                for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
                    unsafe { self.projection_decref(pointer) };
                }
                return false;
            };
            staged.push(pointer);
        }
        let old = {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                drop(handle);
                for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
                    unsafe { self.projection_decref(pointer) };
                }
                return false;
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                drop(handle);
                for pointer in staged.into_iter().filter(|pointer| !pointer.is_null()) {
                    unsafe { self.projection_decref(pointer) };
                }
                return false;
            };
            let old = std::mem::take(&mut allocation.shadow);
            allocation.items = staged;
            allocation.shadow = allocation.items.clone();
            allocation.initialized = next_initialized;
            allocation.uninitialized_count = allocation
                .initialized
                .iter()
                .filter(|value| !**value)
                .count();
            allocation.sealed = allocation.uninitialized_count == 0;
            allocation.publish_storage();
            old
        };
        for pointer in old.into_iter().filter(|pointer| !pointer.is_null()) {
            unsafe { self.projection_decref(pointer) };
        }
        true
    }
}

// Direct list construction slots and stolen-reference publication.
impl ObjectBridge {
    /// Put a freshly allocated list projection into CPython's construction
    /// state: logical size is retained, but every C-visible item slot is NULL.
    pub fn mark_list_view_uninitialized(&self, bits: AbiHandle) -> bool {
        let old = {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return false;
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                return false;
            };
            let old = std::mem::take(&mut allocation.shadow);
            allocation.items.fill(std::ptr::null_mut());
            allocation
                .shadow
                .resize(allocation.items.len(), std::ptr::null_mut());
            allocation.mark_uninitialized();
            old
        };
        for pointer in old.into_iter().filter(|pointer| !pointer.is_null()) {
            unsafe { self.projection_decref(pointer) };
        }
        true
    }

    pub fn list_view_item_initialized(&self, bits: AbiHandle, index: usize) -> Option<bool> {
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        let ManagedView::List { allocation } = &entry.view else {
            return None;
        };
        allocation.initialized.get(index).copied()
    }

    /// Borrow the exact physical object stored in a clean initialized list
    /// slot. C list reads are identity reads, not value rematerialization:
    /// returning a fresh equal numeric carrier here violates `is` and loses the
    /// originating extension object's address. The allocation's projection
    /// edge owns the pointer for the duration of the ordinary CPython borrowed
    /// reference contract.
    pub fn list_view_item_pointer(&self, bits: AbiHandle, index: usize) -> Option<*mut PyObject> {
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        let ManagedView::List { allocation } = &entry.view else {
            return None;
        };
        if allocation.initialized.get(index).copied() != Some(true) {
            return None;
        }
        allocation
            .items
            .get(index)
            .copied()
            .filter(|ptr| !ptr.is_null())
    }

    /// Publish one successful runtime indexed store into the physical list.
    /// `pointer` is the reference stolen by PyList_SetItem; ownership is
    /// transferred directly into the projection rather than decrefing and
    /// rematerializing the complete list. The old projection edge is released
    /// only after the sidecar is coherent, so its finalizer may safely re-enter.
    pub fn publish_list_set_from_stolen(
        &self,
        bits: AbiHandle,
        index: usize,
        pointer: *mut PyObject,
    ) -> bool {
        let old = {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                eprintln!("molt fatal: list view disappeared during indexed publication");
                std::process::abort();
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                eprintln!("molt fatal: list view changed kind during indexed publication");
                std::process::abort();
            };
            let Some(current) = allocation.items.get_mut(index) else {
                eprintln!("molt fatal: runtime accepted an out-of-range list store");
                std::process::abort();
            };
            let old = allocation.shadow[index];
            if *current != old {
                eprintln!("molt fatal: indexed list store began with a dirty direct slot");
                std::process::abort();
            }
            *current = pointer;
            allocation.shadow[index] = pointer;
            let initialized = !pointer.is_null();
            if allocation.initialized[index] != initialized {
                if initialized {
                    allocation.uninitialized_count -= 1;
                } else {
                    allocation.uninitialized_count += 1;
                }
                allocation.initialized[index] = initialized;
            }
            allocation.sealed = allocation.uninitialized_count == 0;
            old
        };
        if !old.is_null() {
            unsafe { self.projection_decref(old) };
        }
        true
    }

    /// Reserve and publish projection-ledger ownership for the reference that
    /// PyList_SetItem will steal. This must happen before the runtime store so
    /// the subsequent physical publication is allocation-free.
    pub unsafe fn prepare_list_set_stolen_ref(&self, pointer: *mut PyObject) -> bool {
        unsafe { self.projection_adopt_owned_ref(pointer) }
    }

    /// Roll back a prepared stolen-reference ledger edge when the runtime store
    /// fails before physical publication. The caller still owns the C
    /// reference, so this changes only ledger state and performs no DECREF.
    pub unsafe fn cancel_list_set_stolen_ref(&self, pointer: *mut PyObject) {
        unsafe { self.projection_unadopt_owned_ref(pointer) };
    }
}

// C-written list projection validation, runtime commit, traversal, and clear.
impl ObjectBridge {
    /// Commit direct stealing writes made through PyListObject.ob_item. Partial
    /// mode supports PyList_SetItem filling a construction list out of order;
    /// complete mode is required at every C-to-runtime observation boundary.
    fn commit_list_view_inner(&self, bits: AbiHandle, require_complete: bool) -> bool {
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        let Some(sync) = ListSyncGuard::enter(bits) else {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"recursive direct PyListObject synchronization".as_ptr(),
                )
            };
            return false;
        };
        let snapshot = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return true;
            };
            let ManagedView::List { allocation } = &entry.view else {
                return true;
            };
            let object = unsafe { &*allocation.object.get() };
            if object.ob_base.ob_size < 0
                || object.ob_base.ob_size as usize != allocation.items.len()
                || object.allocated < object.ob_base.ob_size
                || (allocation.items.is_empty() && !object.ob_item.is_null())
                || (!allocation.items.is_empty()
                    && !std::ptr::eq(object.ob_item, allocation.items.as_ptr().cast_mut()))
            {
                // Error materialization re-enters bridge identity/projection
                // lookup. No Python operation may run under a shard lock.
                drop(handle);
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"invalid direct PyListObject layout state".as_ptr(),
                    )
                };
                return false;
            }
            let mut remaining_uninitialized = allocation.uninitialized_count;
            let mut change_count = 0usize;
            let mut has_null_change = false;
            for ((current, shadow), was_initialized) in allocation
                .items
                .iter()
                .copied()
                .zip(allocation.shadow.iter().copied())
                .zip(allocation.initialized.iter().copied())
            {
                if current != shadow || (!was_initialized && !current.is_null()) {
                    change_count += 1;
                    has_null_change |= current.is_null();
                    if !was_initialized && !current.is_null() {
                        remaining_uninitialized = remaining_uninitialized.saturating_sub(1);
                    }
                }
            }
            let mut changes = Vec::new();
            if change_count != 0 {
                if changes.try_reserve_exact(change_count).is_err() {
                    drop(handle);
                    unsafe { crate::api::errors::PyErr_NoMemory() };
                    return false;
                }
                for (index, ((current, shadow), was_initialized)) in allocation
                    .items
                    .iter()
                    .copied()
                    .zip(allocation.shadow.iter().copied())
                    .zip(allocation.initialized.iter().copied())
                    .enumerate()
                {
                    if current != shadow || (!was_initialized && !current.is_null()) {
                        changes.push((index, current, shadow));
                    }
                }
            }
            (changes, remaining_uninitialized, has_null_change)
        };
        let (changes, remaining_uninitialized, has_null_change) = snapshot;
        if has_null_change {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"direct PyListObject replacement published a NULL item".as_ptr(),
                )
            };
            return false;
        }
        if require_complete && remaining_uninitialized != 0 {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"PyList_New result escaped with uninitialized item slots".as_ptr(),
                )
            };
            return false;
        }
        if changes.is_empty() {
            return true;
        }
        let hooks = crate::hooks::hooks_or_stubs();
        let mut staged: Vec<DirectListCommitCell> = Vec::new();
        if staged.try_reserve_exact(changes.len()).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        }
        for (index, pointer, old_projection) in changes.iter().copied() {
            let new_bits = if let Some(handle) = self.molt_handle_for_pyobj(pointer) {
                unsafe { (hooks.inc_ref)(handle.bits()) };
                handle.bits()
            } else {
                let Some(bits) = (unsafe { self.molt_value_for_pyobj(pointer) }) else {
                    for cell in staged {
                        unsafe { (hooks.dec_ref)(cell.new_bits) };
                    }
                    return false;
                };
                bits
            };
            staged.push(DirectListCommitCell {
                index,
                pointer,
                old_projection,
                new_bits,
                old_bits: None,
                rollback_displaced: None,
            });
        }
        for (adopted, cell) in staged.iter().enumerate() {
            if !unsafe { self.projection_adopt_owned_ref(cell.pointer) } {
                for rollback in staged.iter().take(adopted) {
                    unsafe { self.projection_unadopt_owned_ref(rollback.pointer) };
                }
                for cell in staged {
                    unsafe { (hooks.dec_ref)(cell.new_bits) };
                }
                return false;
            }
        }
        let mut apply_failed = false;
        for cell in &mut staged {
            match unsafe { (hooks.list_set)(bits, cell.index, cell.new_bits) }.decode() {
                crate::hooks::DecodedHandleResult::Ok(old_bits) => {
                    cell.old_bits = Some(old_bits);
                }
                crate::hooks::DecodedHandleResult::Missing
                | crate::hooks::DecodedHandleResult::Error => {
                    apply_failed = true;
                    break;
                }
            }
        }
        if apply_failed {
            for cell in staged.iter_mut().rev() {
                let Some(old_bits) = cell.old_bits else {
                    continue;
                };
                let crate::hooks::DecodedHandleResult::Ok(displaced) =
                    (unsafe { (hooks.list_set)(bits, cell.index, old_bits) }).decode()
                else {
                    eprintln!(
                        "molt fatal: direct list commit rollback failed under the runtime GIL"
                    );
                    std::process::abort();
                };
                cell.rollback_displaced = Some(displaced);
            }
            for cell in &staged {
                unsafe { self.projection_unadopt_owned_ref(cell.pointer) };
            }
            drop(sync);
            for cell in staged {
                if let Some(displaced) = cell.rollback_displaced {
                    unsafe { (hooks.dec_ref)(displaced) };
                }
                if let Some(old_bits) = cell.old_bits {
                    unsafe { (hooks.dec_ref)(old_bits) };
                }
                unsafe { (hooks.dec_ref)(cell.new_bits) };
            }
            unsafe { ensure_result_error(c"runtime list snapshot commit failed") };
            return false;
        }

        {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                eprintln!("molt fatal: list view disappeared during direct commit");
                std::process::abort();
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                eprintln!("molt fatal: list view changed kind during direct commit");
                std::process::abort();
            };
            for cell in &staged {
                if allocation.items[cell.index] != cell.pointer
                    || allocation.shadow[cell.index] != cell.old_projection
                {
                    eprintln!("molt fatal: direct list slots changed under the runtime GIL");
                    std::process::abort();
                }
                allocation.shadow[cell.index] = cell.pointer;
                if !allocation.initialized[cell.index] {
                    allocation.uninitialized_count =
                        allocation.uninitialized_count.saturating_sub(1);
                    allocation.initialized[cell.index] = true;
                }
            }
            allocation.sealed = allocation.uninitialized_count == 0;
        }

        // The two authorities are now coherent. Release displaced edges only
        // after dropping the recursion guard so arbitrary finalizers can
        // re-enter and observe the clean list.
        drop(sync);
        for cell in staged {
            if !cell.old_projection.is_null() {
                unsafe { self.projection_decref(cell.old_projection) };
            }
            unsafe {
                (hooks.dec_ref)(cell.new_bits);
                (hooks.dec_ref)(
                    cell.old_bits
                        .expect("successful list commit missing old value"),
                );
            }
        }
        true
    }
}

// Completed list projection commit wrappers, GC traversal, and clear.
impl ObjectBridge {
    pub fn commit_list_view_partial(&self, bits: AbiHandle) -> bool {
        self.commit_list_view_inner(bits, false)
    }

    pub fn commit_list_view(&self, bits: AbiHandle) -> bool {
        self.commit_list_view_inner(bits, true)
    }

    /// Visit independently owned physical C edges in the mixed GC graph.
    /// Exception members and CFunction.m_module are ordinary C owners. Clean
    /// list slots mirror runtime edges; only dirty direct C writes add an edge.
    /// Copy pointers under the parent lock and classify after dropping it so
    /// self/same-shard edges cannot deadlock. Fixed layouts stay on the stack.
    pub fn visit_physical_owned_edges_for_gc(
        &self,
        bits: AbiHandle,
        visit: &mut dyn FnMut(crate::NativeGcEdge),
    ) {
        enum Fields {
            Fixed([*mut PyObject; EXCEPTION_VIEW_POINTER_FIELDS]),
            List(Vec<*mut PyObject>),
        }

        let fields = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return;
            };
            match &entry.view {
                ManagedView::Exception(allocation) => {
                    Fields::Fixed(unsafe { allocation.state().pointers() })
                }
                ManagedView::CFunction(object) => {
                    let mut fields = [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS];
                    fields[0] = unsafe { (*object.get()).func.m_module };
                    Fields::Fixed(fields)
                }
                ManagedView::List { allocation } => Fields::List(
                    allocation
                        .items
                        .iter()
                        .copied()
                        .zip(allocation.shadow.iter().copied())
                        .filter_map(|(current, shadow)| {
                            (current != shadow && !current.is_null()).then_some(current)
                        })
                        .collect(),
                ),
                _ => return,
            }
        };
        let fields: &[*mut PyObject] = match &fields {
            Fields::Fixed(fields) => fields,
            Fields::List(fields) => fields,
        };
        for &field in fields {
            if let Some(edge) = crate::NativeGcEdge::from_pyobj(self, field) {
                visit(edge);
            }
        }
    }

    /// Publish an empty list projection before releasing its C ownership edges.
    pub fn clear_list_view(&self, bits: AbiHandle) -> Option<RetiredClearedListProjection> {
        let (items, shadow) = {
            let mut handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get_mut(&bits)?;
            let ManagedView::List { allocation } = &mut entry.view else {
                return None;
            };
            let items = std::mem::take(&mut allocation.items);
            let shadow = std::mem::take(&mut allocation.shadow);
            let initialized = std::mem::take(&mut allocation.initialized);
            allocation.uninitialized_count = 0;
            allocation.sealed = true;
            allocation.publish_storage();
            drop(initialized);
            (items, shadow)
        };
        debug_assert_eq!(items.len(), shadow.len());
        Some(RetiredClearedListProjection { items, shadow })
    }
}

fn release_exception_snapshot_handles(snapshot: &crate::hooks::ExceptionSnapshot) {
    let hooks = crate::hooks::hooks_or_stubs();
    for bits in snapshot.present_handles() {
        unsafe { (hooks.dec_ref)(bits) };
    }
}

// Physical exception projection refresh, GC traversal, direct-write commit, and clear.
impl ObjectBridge {
    /// Pull the complete runtime exception state into its physical
    /// `PyBaseExceptionObject` in one publication.  The hook pins every field;
    /// conversion happens before the bridge lock and old C references are
    /// released only after the atomic pointer swap.
    pub fn refresh_exception_view(&self, bits: AbiHandle) -> bool {
        let expected_layout = {
            let handle = self.handle_shard(bits).lock();
            match handle.to_py.get(&bits).map(|entry| &entry.view) {
                Some(ManagedView::Exception(allocation)) => Some(allocation.layout_kind()),
                _ => None,
            }
        };
        let Some(expected_layout) = expected_layout else {
            return true;
        };
        let Some(_sync) = ExceptionSyncGuard::enter(bits) else {
            return true;
        };
        let hooks = crate::hooks::hooks_or_stubs();
        let mut snapshot = crate::hooks::ExceptionSnapshot::default();
        if unsafe { (hooks.exception_snapshot)(bits, &raw mut snapshot) } != 0 {
            unsafe { ensure_result_error(c"runtime exception snapshot failed") };
            return false;
        }
        let Some(layout_kind) = snapshot.validated_layout(Some(expected_layout)) else {
            release_exception_snapshot_handles(&snapshot);
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"runtime returned malformed or mismatched typed exception state".as_ptr(),
                )
            };
            return false;
        };

        let mut next = ExceptionViewState::empty(layout_kind);
        next.suppress_context = snapshot.suppress_context as std::os::raw::c_char;
        next.unicode_start = snapshot.unicode_start;
        next.unicode_end = snapshot.unicode_end;
        next.os_error_written = snapshot.os_error_written;
        let mut fields = snapshot
            .base_fields()
            .into_iter()
            .chain(snapshot.typed_fields());
        let converted = next
            .base
            .iter_mut()
            .chain(next.typed.iter_mut())
            .try_for_each(|slot| {
                let Some(handle_bits) = fields
                    .next()
                    .expect("snapshot and physical field counts agree")
                else {
                    return Ok(());
                };
                let field = unsafe { self.owned_handle_to_pyobj(handle_bits) };
                if field.is_null() {
                    return Err(());
                }
                *slot = field;
                Ok(())
            });
        if converted.is_err() {
            drop(RetiredOwnedCFields::from(next));
            for handle_bits in fields.flatten() {
                unsafe { (hooks.dec_ref)(handle_bits) };
            }
            return false;
        }
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            drop(handle);
            drop(RetiredOwnedCFields::from(next));
            return false;
        };
        let ManagedView::Exception(allocation) = &mut entry.view else {
            drop(handle);
            drop(RetiredOwnedCFields::from(next));
            return false;
        };
        let Some(old) = (unsafe { allocation.replace_state(next) }) else {
            drop(handle);
            drop(RetiredOwnedCFields::from(next));
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"typed exception projection layout changed during refresh".as_ptr(),
                )
            };
            return false;
        };
        drop(handle);
        drop(RetiredOwnedCFields::from(old));
        true
    }
}

// Ordinary physical C-field clear and exception snapshot commit.
impl ObjectBridge {
    /// Detach mutable Type mirrors after their semantic slots have become empty.
    /// Keep identity published and release the returned owners outside locks.
    pub fn clear_type_view_cycle_edges(
        &self,
        bits: AbiHandle,
    ) -> Option<RetiredTypeCycleProjection> {
        let mut handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get_mut(&bits)?;
        if !matches!(entry.view, ManagedView::Type { .. }) {
            return None;
        }
        let mut pointers = [std::ptr::null_mut(); 2];
        let mut count = 0;
        entry
            .view
            .owned_items_with(true, true, |pointer, mirrored| {
                assert!(mirrored);
                if !pointer.is_null() {
                    pointers[count] = pointer;
                    count += 1;
                }
            });
        Some(RetiredTypeCycleProjection { pointers })
    }

    /// Detach ordinary C-owned physical fields after the runtime payload has
    /// published its cleared state. The returned guard owns every displaced
    /// reference; callbacks run only when the caller releases detached resources
    /// outside bridge locks. Slot mirrors retain their terminal release order.
    /// Null publication makes repeated clear and terminal teardown idempotent.
    pub fn clear_owned_c_fields(&self, bits: AbiHandle) -> Option<RetiredOwnedCFields> {
        let old = {
            let mut handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get_mut(&bits)?;
            match &mut entry.view {
                ManagedView::Exception(allocation) => {
                    let empty = ExceptionViewState::empty(allocation.layout_kind());
                    unsafe { allocation.replace_state(empty)? }.pointers()
                }
                ManagedView::CFunction(object) => {
                    let mut pointers = [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS];
                    pointers[0] = unsafe {
                        std::mem::replace(&mut (*object.get()).func.m_module, std::ptr::null_mut())
                    };
                    pointers
                }
                _ => return None,
            }
        };
        Some(RetiredOwnedCFields { pointers: old })
    }

    /// Commit direct C writes to a managed `PyBaseExceptionObject` before the
    /// runtime observes that exception again.  Every C pointer is temporarily
    /// pinned, converted to an owned runtime handle, validated as a complete
    /// snapshot, and only then published by the runtime hook.
    pub fn commit_exception_view(&self, bits: AbiHandle) -> bool {
        let Some(_sync) = ExceptionSyncGuard::enter(bits) else {
            return true;
        };
        let (state, ob_type) = loop {
            let captured = {
                let handle = self.handle_shard(bits).lock();
                let Some(entry) = handle.to_py.get(&bits) else {
                    return true;
                };
                let ManagedView::Exception(allocation) = &entry.view else {
                    return true;
                };
                unsafe { (allocation.state(), allocation.base().ob_base.ob_type) }
            };
            for field in captured
                .0
                .pointers()
                .into_iter()
                .filter(|field| !field.is_null())
            {
                unsafe { crate::api::refcount::Py_INCREF(field) };
            }
            let current = {
                let handle = self.handle_shard(bits).lock();
                handle.to_py.get(&bits).and_then(|entry| {
                    let ManagedView::Exception(allocation) = &entry.view else {
                        return None;
                    };
                    unsafe { Some((allocation.state(), allocation.base().ob_base.ob_type)) }
                })
            };
            if current == Some(captured) {
                break captured;
            }
            for field in captured
                .0
                .pointers()
                .into_iter()
                .filter(|field| !field.is_null())
            {
                unsafe { crate::api::refcount::Py_DECREF(field) };
            }
            if current.is_none() {
                return true;
            }
        };
        let pinned_pointers = state.pointers();
        if state.base[1].is_null()
            || !matches!(state.suppress_context, 0 | 1)
            || ob_type.is_null()
            || unsafe { (*ob_type).tp_basicsize }
                != crate::abi_types::exception_layout_basicsize(state.layout_kind)
        {
            for field in pinned_pointers.into_iter().filter(|field| !field.is_null()) {
                unsafe { crate::api::refcount::Py_DECREF(field) };
            }
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"invalid direct typed exception object state".as_ptr(),
                )
            };
            return false;
        }
        let hooks = crate::hooks::hooks_or_stubs();
        let mut snapshot = crate::hooks::ExceptionSnapshot {
            layout_kind: state.layout_kind as u8,
            suppress_context: state.suppress_context as u32,
            unicode_start: state.unicode_start,
            unicode_end: state.unicode_end,
            os_error_written: state.os_error_written,
            ..crate::hooks::ExceptionSnapshot::default()
        };
        let mut converted = true;
        let mut runtime_base = [0u64; EXCEPTION_BASE_POINTER_FIELDS];
        for (index, field) in state.base.iter().copied().enumerate() {
            if field.is_null() {
                continue;
            }
            let Some(value_bits) = (unsafe { self.molt_value_for_pyobj(field) }) else {
                converted = false;
                break;
            };
            runtime_base[index] = value_bits;
            snapshot.present_mask |= crate::hooks::EXCEPTION_BASE_FIELD_MASKS[index];
        }
        if converted {
            for (index, field) in state
                .typed
                .iter()
                .copied()
                .take(state.layout_kind.field_policies().len())
                .enumerate()
            {
                if field.is_null() {
                    continue;
                }
                let Some(value_bits) = (unsafe { self.molt_value_for_pyobj(field) }) else {
                    converted = false;
                    break;
                };
                snapshot.typed_handles[index] = value_bits;
                snapshot.typed_present_mask |= 1 << index;
            }
        }
        snapshot.dict = runtime_base[0];
        snapshot.args = runtime_base[1];
        snapshot.notes = runtime_base[2];
        snapshot.traceback = runtime_base[3];
        snapshot.context = runtime_base[4];
        snapshot.cause = runtime_base[5];
        let structurally_valid =
            converted && snapshot.validated_layout(Some(state.layout_kind)).is_some();
        let committed = structurally_valid
            && unsafe { (hooks.exception_commit_snapshot)(bits, &raw const snapshot) } == 0;
        release_exception_snapshot_handles(&snapshot);
        for field in pinned_pointers.into_iter().filter(|field| !field.is_null()) {
            unsafe { crate::api::refcount::Py_DECREF(field) };
        }
        if !committed {
            unsafe { ensure_result_error(c"managed exception snapshot commit failed") };
        }
        committed
    }
}

// Bidirectional identity, foreign-wrapper custody, and static registration.
impl ObjectBridge {
    fn pyobj_matches_handle(&self, ptr: *mut PyObject, bits: AbiHandle) -> bool {
        if ptr.is_null() {
            return false;
        }
        if self.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle) == Some(bits) {
            return true;
        }
        let address = self.address_shard(ptr.addr()).lock();
        address.foreign.get(&ptr.addr()).copied() == Some(bits)
    }

    pub fn molt_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<MoltValueHandle> {
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            return Some(MoltValueHandle(bits));
        }
        let addr = ptr.addr();
        let address = self.address_shard(addr).lock();
        if let Some(bits) = address
            .numeric_carriers
            .get(&addr)
            .and_then(|record| record.bits)
        {
            return Some(MoltValueHandle(bits));
        }
        if let Some(bits) = address.direct_molt_py.get(&addr).copied() {
            return Some(MoltValueHandle(bits));
        }
        drop(address);
        self.managed_handle_for_pyobj(ptr).map(MoltValueHandle)
    }

    /// Resolve a C object for semantic runtime observation. Unlike the raw
    /// identity lookup, this commits every mutable physical projection first,
    /// so generic protocols cannot observe stale list/exception state merely
    /// because they bypassed a type-specific C API entry point.
    pub fn observed_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<MoltValueHandle> {
        let value = self.molt_handle_for_pyobj(ptr)?;
        self.prepare_runtime_value(value, RuntimeValueAccess::Observe)
    }

    fn prepare_runtime_value(
        &self,
        value: MoltValueHandle,
        access: RuntimeValueAccess,
    ) -> Option<MoltValueHandle> {
        if !observe_publication(self, value.bits()) {
            unsafe { ensure_result_error(c"managed projection is retiring") };
            return None;
        }
        match Self::classify_handle(value.bits()) {
            MoltTypeTag::Str if !self.commit_unicode_view(value.bits()) => return None,
            MoltTypeTag::Exception if !self.commit_exception_view(value.bits()) => return None,
            MoltTypeTag::List
                if !self.commit_list_view_inner(
                    value.bits(),
                    matches!(access, RuntimeValueAccess::Observe),
                ) =>
            {
                return None;
            }
            _ => {}
        }
        Some(value)
    }

    /// Resolve only the canonical ABI view owned by a live Molt heap object.
    /// Static singletons, scalar layout carriers, and foreign objects are not
    /// managed views and retain their native deallocation authority.
    pub fn managed_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        if ptr.is_null() {
            return None;
        }
        let addr = ptr.addr();
        loop {
            let address = self.address_shard(addr).lock();
            let bits = address.from_py.get(&addr).copied()?;
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let entry = handle.to_py.get(&bits)?;
            if entry.view.py_obj().addr() != addr {
                return None;
            }
            match &entry.publication {
                PublicationState::Ready => return Some(bits),
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                    return Some(bits);
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                }
            }
        }
    }

    pub unsafe fn molt_value_for_pyobj(&self, ptr: *mut PyObject) -> Option<u64> {
        if ptr.is_null() {
            return None;
        }
        unsafe { self.acquire_runtime_value(ptr, RuntimeValueAccess::Observe) }
            .map(RuntimeValue::into_owned_bits)
    }

    unsafe fn acquire_runtime_value(
        &self,
        ptr: *mut PyObject,
        access: RuntimeValueAccess,
    ) -> Option<RuntimeValue> {
        if let Some(value) = self.molt_handle_for_pyobj(ptr) {
            // A failed semantic commit is not evidence of foreign identity.
            return self
                .prepare_runtime_value(value, access)
                .map(|value| RuntimeValue {
                    bits: value.bits(),
                    owned: false,
                });
        }
        admit_foreign_pyobject(ptr)?;
        #[cfg(test)]
        static_binding_transaction_tests::pause_before_foreign_reservation();
        unsafe { self.foreign_wrapper_for(ptr, access) }
    }

    unsafe fn foreign_wrapper_for(
        &self,
        ptr: *mut PyObject,
        access: RuntimeValueAccess,
    ) -> Option<RuntimeValue> {
        let key = ptr.expose_provenance();
        let hooks = crate::hooks::hooks_or_stubs();
        let address_index = self.address_shard_index(key);
        let mut address = self.address_shards[address_index].lock();
        loop {
            // Canonical publication may have won after the optimistic lookup
            // but before this reservation. Once reserved, static raw
            // publication rejects foreign_inflight under this same lock.
            if address.from_py.contains_key(&key)
                || address.direct_molt_py.contains_key(&key)
                || address.numeric_carriers.contains_key(&key)
            {
                drop(address);
                let value = self
                    .molt_handle_for_pyobj(ptr)
                    .and_then(|value| self.prepare_runtime_value(value, access));
                if let Some(value) = value {
                    return Some(RuntimeValue {
                        bits: value.bits(),
                        owned: false,
                    });
                }
                unsafe {
                    ensure_result_error(c"registered ABI object could not yield its runtime value")
                };
                return None;
            }
            if let Some(wrapper) = address.foreign.get(&key).copied() {
                drop(address);
                unsafe { (hooks.inc_ref)(wrapper) };
                return Some(RuntimeValue {
                    bits: wrapper,
                    owned: true,
                });
            }
            if address.foreign_inflight.insert(key) {
                break;
            }
            self.foreign_ready[address_index].wait(&mut address);
        }
        drop(address);

        let wrapper = unsafe { (hooks.foreign_new)(key) };
        if wrapper == 0 {
            let mut address = self.address_shards[address_index].lock();
            address.foreign_inflight.remove(&key);
            self.foreign_ready[address_index].notify_all();
            drop(address);
            unsafe { ensure_result_error(c"foreign runtime value acquisition failed") };
            return None;
        }

        // Acquire the C custody edge before taking bridge publication locks.
        // Py_INCREF probes managed membership and therefore re-enters the
        // address shard; doing it under `lock_address_then_handle` deadlocks on
        // the first genuine foreign crossing.
        unsafe { crate::api::refcount::Py_INCREF(ptr) };
        let (mut address, mut handle) = self.lock_address_then_handle(key, wrapper);
        if handle.raw_py.contains_key(&wrapper) || handle.to_py.contains_key(&wrapper) {
            address.foreign_inflight.remove(&key);
            self.foreign_ready[address_index].notify_all();
            drop(handle);
            drop(address);
            // A faulty producer must not replace another binding or abandon
            // either the returned owned wrapper or the C custody edge.
            crate::api::errors::with_preserved_error(|| unsafe { (hooks.dec_ref)(wrapper) });
            unsafe { crate::api::errors::release_preserving_error(&[ptr]) };
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                    c"foreign wrapper producer returned an existing bridge identity".as_ptr(),
                );
            }
            return None;
        }
        address.foreign.insert(key, wrapper);
        address.foreign_inflight.remove(&key);
        handle.raw_py.insert(wrapper, RawBinding::borrowed(key));
        self.foreign_ready[address_index].notify_all();
        Some(RuntimeValue {
            bits: wrapper,
            owned: true,
        })
    }

    pub unsafe fn release_foreign(&self, c_ptr: usize) {
        let mut address = self.address_shard(c_ptr).lock();
        let Some(wrapper) = address.foreign.get(&c_ptr).copied() else {
            return;
        };
        let mut handle = self.handle_shard(wrapper).lock();
        address.foreign.remove(&c_ptr);
        if handle.raw_py.get(&wrapper) == Some(&RawBinding::borrowed(c_ptr)) {
            handle.raw_py.remove(&wrapper);
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_foreign_for_test(&self, ptr: *mut PyObject, handle_bits: AbiHandle) {
        let exposed_addr = ptr.expose_provenance();
        let (mut address, mut handle) = self.lock_address_then_handle(ptr.addr(), handle_bits);
        address.foreign.insert(ptr.addr(), handle_bits);
        handle
            .raw_py
            .insert(handle_bits, RawBinding::borrowed(exposed_addr));
    }

    /// Rebind a canonical static C object (notably `PyExc_*`) from its
    /// bootstrap binding to a real runtime handle. Ingress always
    /// uses `direct_molt_py`; when `canonical_view` is true, handle-to-PyObject
    /// projection also resolves to this immortal static pointer. Publication
    /// atomically validates and updates both directions under the address and
    /// affected handle locks; a canonical reverse binding cannot replace an
    /// existing managed view or another raw pointer.
    pub unsafe fn bind_static_pyobj_to_runtime_handle(
        &self,
        ptr: *mut PyObject,
        bits: AbiHandle,
        canonical_view: bool,
    ) -> Result<(), StaticBindingError> {
        if ptr.is_null() || bits == 0 {
            return Err(StaticBindingError::InvalidInput {
                address: ptr.addr(),
                bits,
            });
        }
        let addr = ptr.addr();
        let mut address = self.address_shard(addr).lock();
        let old_bits = address.from_py.get(&addr).copied();
        // A static identity may replace only an exact direct binding, never
        // a managed view, numeric carrier, or foreign-wrapper identity.
        if address.direct_molt_py.get(&addr).copied() != old_bits
            || address.foreign.contains_key(&addr)
            || address.foreign_inflight.contains(&addr)
            || address.numeric_carriers.contains_key(&addr)
        {
            return Err(StaticBindingError::AddressIdentityConflict {
                forward: old_bits,
                direct: address.direct_molt_py.get(&addr).copied(),
                foreign: address.foreign.get(&addr).copied(),
                foreign_inflight: address.foreign_inflight.contains(&addr),
                numeric_carrier: address
                    .numeric_carriers
                    .get(&addr)
                    .map(|record| record.bits),
            });
        }
        let target_index = self.handle_shard_index(bits);
        let old_index = old_bits.map_or(target_index, |old| self.handle_shard_index(old));
        // Address rank first, then each affected handle rank exactly once in
        // index order. Validation and every map edit share this lock set.
        let first_index = old_index.min(target_index);
        let second_index = old_index.max(target_index);
        let mut first = self.handle_shards[first_index].lock();
        let mut second =
            (first_index != second_index).then(|| self.handle_shards[second_index].lock());
        {
            let target = if target_index == first_index {
                &*first
            } else {
                second.as_deref().unwrap()
            };
            let raw = target.raw_py.get(&bits).copied();
            if canonical_view {
                if let Some(address) = target.managed_address(bits) {
                    return Err(StaticBindingError::CanonicalTargetManaged { address });
                }
                if let Some(binding) = raw
                    && binding != RawBinding::borrowed(addr)
                {
                    return Err(StaticBindingError::CanonicalTargetBorrowed {
                        address: binding.address,
                    });
                }
            }
        }
        if let Some(old_bits) = old_bits {
            let old = if old_index == first_index {
                &mut *first
            } else {
                second.as_deref_mut().unwrap()
            };
            if old.managed_address(old_bits) == Some(addr) {
                return Err(StaticBindingError::ManagedPrevious {
                    bits: old_bits,
                    address: addr,
                });
            }
            if old_bits != bits && old.raw_py.get(&old_bits) == Some(&RawBinding::borrowed(addr)) {
                old.raw_py.remove(&old_bits);
            }
        }
        address.from_py.insert(addr, bits);
        address.direct_molt_py.insert(addr, bits);
        #[cfg(test)]
        static_binding_transaction_tests::pause_after_forward_publication();
        if canonical_view {
            let target = if target_index == first_index {
                &mut *first
            } else {
                second.as_deref_mut().unwrap()
            };
            target.raw_py.insert(bits, RawBinding::borrowed(addr));
        }
        Ok(())
    }

    /// Retire one exact static/direct C-object binding without changing any
    /// reference ownership.
    ///
    /// Static bindings occupy both address-keyed ingress maps.  Requiring the
    /// supplied pair to match in both maps and excluding an exact managed
    /// address protects managed views. Conditional reverse removal preserves a
    /// newer canonical pointer or an independent alias for the same runtime handle. The
    /// runtime owns any strong class anchor associated with this identity and
    /// must retire that anchor separately after unbinding.
    ///
    /// Returns `false` without mutation for null/zero inputs or when the exact
    /// forward pair is no longer current.  A missing reverse mapping is valid:
    /// bindings created with `canonical_view == false` intentionally have no
    /// handle-to-pointer entry.
    pub unsafe fn unbind_static_pyobj_from_runtime_handle(
        &self,
        ptr: *mut PyObject,
        bits: AbiHandle,
    ) -> bool {
        if ptr.is_null() || bits == 0 {
            return false;
        }
        let addr = ptr.addr();
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        if address.from_py.get(&addr).copied() != Some(bits)
            || address.direct_molt_py.get(&addr).copied() != Some(bits)
            || handle.managed_address(bits) == Some(addr)
        {
            return false;
        }
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        if handle.raw_py.get(&bits) == Some(&RawBinding::borrowed(addr)) {
            handle.raw_py.remove(&bits);
        }
        true
    }

    pub(crate) fn register_numeric_carrier(
        &self,
        ptr: *mut PyObject,
        bits: Option<AbiHandle>,
        kind: NumericCarrierKind,
    ) {
        if ptr.is_null() {
            return;
        }
        self.address_shard(ptr.addr())
            .lock()
            .numeric_carriers
            .insert(ptr.addr(), NumericCarrierRecord { bits, kind });
    }
}

// Projection-owned C-reference accounting and numeric carrier retirement.
impl ObjectBridge {
    /// Private C mirrors already represented by runtime graph ownership.
    /// Managed and native GC nodes discount this same ledger; independently
    /// traversed C fields never enter it.
    pub fn mirrored_c_refcount(&self, address: usize) -> usize {
        self.address_shard(address)
            .lock()
            .projection_refs
            .get(&address)
            .copied()
            .unwrap_or(0)
    }

    /// Retain a private mirror already represented in the runtime ownership
    /// graph. C-writable fields (exception members and CFunction.m_module) own
    /// ordinary C references and are visited independently by shared cycle GC.
    unsafe fn projection_incref(&self, ptr: *mut PyObject) -> bool {
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            return true;
        }
        if !unsafe { self.projection_adopt_owned_ref(ptr) } {
            return false;
        }
        unsafe { crate::api::refcount::Py_INCREF(ptr) };
        true
    }

    /// Adopt an already-owned private mirror without incrementing it again.
    unsafe fn projection_adopt_owned_ref(&self, ptr: *mut PyObject) -> bool {
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            return true;
        }
        let mut address = self.address_shard(ptr.addr()).lock();
        let count = address
            .projection_refs
            .get(&ptr.addr())
            .copied()
            .unwrap_or(0);
        let Some(next) = count.checked_add(1) else {
            drop(address);
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        };
        if count == 0 && address.projection_refs.try_reserve(1).is_err() {
            drop(address);
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        }
        address.projection_refs.insert(ptr.addr(), next);
        true
    }

    /// Undo adoption without consuming the C reference during staging rollback.
    unsafe fn projection_unadopt_owned_ref(&self, ptr: *mut PyObject) {
        if ptr.is_null() {
            return;
        }
        let mut address = self.address_shard(ptr.addr()).lock();
        if unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            // Promotion can occur after these mirrors were retained. Their
            // numeric ownership is now absorbed by the process-long C root.
            address.projection_refs.remove(&ptr.addr());
            return;
        }
        let remove = match address.projection_refs.get_mut(&ptr.addr()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                *count == 0
            }
            _ => {
                eprintln!(
                    "molt fatal: missing or invalid mirrored projection reference ledger entry"
                );
                std::process::abort();
            }
        };
        if remove {
            address.projection_refs.remove(&ptr.addr());
        }
    }

    /// Release one projection-owned C edge. Publish the ledger decrement before
    /// Py_DECREF can enter terminal/refcount-zero logic.
    unsafe fn projection_decref(&self, ptr: *mut PyObject) {
        if ptr.is_null() {
            return;
        }
        unsafe { self.projection_unadopt_owned_ref(ptr) };
        unsafe { crate::api::refcount::Py_DECREF(ptr) };
    }

    pub(crate) fn unregister_numeric_carrier(
        &self,
        ptr: *mut PyObject,
    ) -> Option<NumericCarrierRecord> {
        if ptr.is_null() {
            return None;
        }
        self.address_shard(ptr.addr())
            .lock()
            .numeric_carriers
            .remove(&ptr.addr())
    }
}

/// Clinic's leading name/signature plus separator is metadata, not public
/// doc text. Non-Clinic documentation remains byte-for-byte unchanged.
fn cfunction_clinic_document<'a>(name: &[u8], document: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    let suffix = document.strip_prefix(name)?;
    if suffix.first() != Some(&b'(') {
        return None;
    }
    let end = suffix.windows(5).position(|window| window == b"\n--\n\n")?;
    let signature = &suffix[..end];
    if signature.last() != Some(&b')') {
        return None;
    }
    Some((signature, &suffix[end + 5..]))
}

// Concrete C callable publication and GC traversal.
impl ObjectBridge {
    /// Runtime callable carriers have actual vectorcall storage even though
    /// their physical header intentionally does not claim a native C function.
    pub(crate) fn runtime_vectorcall(
        &self,
        object: *mut PyObject,
    ) -> Option<crate::abi_types::PyVectorcallFunc> {
        let bits = self.managed_handle_for_pyobj(object)?;
        let handle = self.handle_shard(bits).lock();
        let ManagedView::RuntimeCallable(callable) = &handle.to_py.get(&bits)?.view else {
            return None;
        };
        unsafe { (*callable.get()).func.vectorcall }
    }

    /// Publish a fresh runtime C callable as its canonical concrete view and
    /// return one new C reference. `object` is the complete
    /// `PyCFunctionObject`/`PyCMethodObject` layout; its `m_self`, `m_module`
    /// and `mm_class` are borrowed from the caller and adopted here.
    ///
    /// The owned runtime `bits` becomes the stable view hold on success and is
    /// released on failure. Releasing the constructor C reference therefore
    /// retires nothing while any runtime owner (for example a module dict)
    /// remains; the physical object, `m_ml` and all member edges persist until
    /// runtime terminal retirement runs `release_owned_items`. `m_self` and
    /// `mm_class` mirror the runtime callable closure, so like clean tuple
    /// items they are untraversed projection edges and a module/function
    /// cycle stays collectable. `m_module` has no runtime mirror and is an
    /// independent edge exposed by `visit_physical_owned_edges_for_gc`.
    pub unsafe fn publish_cfunction_view(
        &self,
        bits: AbiHandle,
        object: PyCMethodObject,
    ) -> *mut PyObject {
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        let Some(build_guard) = PublicationBuildGuard::enter(self, bits) else {
            crate::api::errors::with_preserved_error(|| unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            });
            return std::ptr::null_mut();
        };
        let release_entry = |entry: &mut BridgeEntry| {
            crate::api::errors::with_preserved_error(|| unsafe {
                entry.view.release_owned_items();
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            })
        };
        let edges = [
            (object.func.m_self, true),
            (object.func.m_module, false),
            (object.mm_class.cast::<PyObject>(), true),
        ];
        let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
        let (initial_refs, has_non_view_runtime_owner) =
            initial_managed_view_refs(runtime_refs, true);
        let mut object = object;
        object.func.ob_base.ob_refcnt = if crate::abi_types::is_immortal_refcnt(initial_refs) {
            initial_refs
        } else {
            initial_refs + 1
        };
        object.func.m_self = std::ptr::null_mut();
        object.func.m_module = std::ptr::null_mut();
        object.mm_class = std::ptr::null_mut();
        let mut entry = Box::new(BridgeEntry {
            view: ManagedView::CFunction(Box::new(UnsafeCell::new(object))),
            bits,
            unicode: None,
            publication: PublicationState::Building {
                owner: std::thread::current().id(),
            },
            lifecycle: if has_non_view_runtime_owner {
                BridgeLifecycle::RuntimeOwned
            } else {
                BridgeLifecycle::ViewHoldOnly
            },
        });
        let ptr = entry.view.py_obj();
        let physical = ptr.cast::<PyCMethodObject>();
        let slots = unsafe {
            [
                &raw mut (*physical).func.m_self,
                &raw mut (*physical).func.m_module,
                (&raw mut (*physical).mm_class).cast::<*mut PyObject>(),
            ]
        };
        for (slot, (edge, mirrored)) in slots.into_iter().zip(edges) {
            // These C fields arrive as raw borrowed pointers, so their existing
            // provisional dependencies need not pass through published_pyobj.
            // Retaining an opaque field must not commit its mutable projection
            // or require an open construction container to be observable.
            if let Some(value) = self.managed_handle_for_pyobj(edge)
                && !observe_publication(self, value)
            {
                release_entry(&mut entry);
                return std::ptr::null_mut();
            }
            if mirrored {
                if !unsafe { self.projection_incref(edge) } {
                    release_entry(&mut entry);
                    return std::ptr::null_mut();
                }
            } else {
                unsafe { crate::api::refcount::Py_XINCREF(edge) };
            }
            // Rollback owns only slots whose retain completed successfully.
            unsafe { *slot = edge };
        }
        match self.insert_managed_entry(bits, entry) {
            Ok(()) => {
                build_guard.inserted(ptr);
                if build_guard.finish() {
                    ptr
                } else {
                    unsafe {
                        ensure_result_error(c"recursive C callable publication failed");
                    }
                    std::ptr::null_mut()
                }
            }
            Err((mut entry, rejection)) => {
                release_entry(&mut entry);
                unsafe { rejection.set_error() };
                std::ptr::null_mut()
            }
        }
    }

    /// CPython's internal Clinic document owns both public documentation and
    /// text signature. Copy bytes only while borrowing its canonical C view.
    pub fn cfunction_metadata(&self, bits: AbiHandle, signature: bool) -> Option<Option<Vec<u8>>> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        let ManagedView::CFunction(object) = &entry.view else {
            return None;
        };
        let method = unsafe { (*object.get()).func.m_ml.as_ref() }?;
        if method.ml_doc.is_null() {
            return Some(None);
        }
        let document = unsafe { std::ffi::CStr::from_ptr(method.ml_doc).to_bytes() };
        let name = (!method.ml_name.is_null())
            .then(|| unsafe { std::ffi::CStr::from_ptr(method.ml_name).to_bytes() });
        let parsed = name.and_then(|name| cfunction_clinic_document(name, document));
        Some(if signature {
            parsed.map(|(signature, _)| signature.to_vec())
        } else {
            Some(parsed.map_or(document, |(_, body)| body).to_vec())
        })
    }

    /// The receiver belongs to the C method/context, never to a second public
    /// function-dictionary edge. The returned runtime handle is owned.
    pub fn cfunction_self(&self, bits: AbiHandle) -> Option<Result<AbiHandle, ()>> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let receiver = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            let ManagedView::CFunction(object) = &entry.view else {
                return None;
            };
            unsafe { (*object.get()).func.m_self }
        };
        Some(if receiver.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            unsafe { RuntimeValue::acquire(receiver) }
                .map(RuntimeValue::into_owned_bits)
                .ok_or(())
        })
    }

    /// The physical C member is the sole module-metadata authority for these
    /// callables. None means this value has no CFunction view; an error keeps
    /// the C exception intact for the runtime boundary to transfer.
    pub fn cfunction_module(&self, bits: AbiHandle) -> Option<Result<AbiHandle, ()>> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let module = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            let ManagedView::CFunction(object) = &entry.view else {
                return None;
            };
            unsafe { (*object.get()).func.m_module }
        };
        Some(if module.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            unsafe { RuntimeValue::acquire(module) }
                .map(RuntimeValue::into_owned_bits)
                .ok_or(())
        })
    }

    /// Replace a CFunction's module edge transactionally. Deletion clears the
    /// physical pointer (reads produce None); assigning None retains Py_None.
    /// New ownership is staged before publication; old ownership retires only
    /// after the handle lock is released, since finalizers may re-enter.
    pub fn set_cfunction_module(&self, bits: AbiHandle, value: Option<AbiHandle>) -> Option<bool> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        {
            let handle = self.handle_shard(bits).lock();
            if !matches!(&handle.to_py.get(&bits)?.view, ManagedView::CFunction(_)) {
                return None;
            }
        }
        let pointer = if let Some(value) = value {
            let pointer = unsafe { self.borrowed_handle_to_new_pyobj(value) };
            if pointer.is_null() {
                return Some(false);
            }
            pointer
        } else {
            std::ptr::null_mut()
        };
        let old = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle
                .to_py
                .get(&bits)
                .expect("caller retains CFunction owner");
            let ManagedView::CFunction(object) = &entry.view else {
                unreachable!()
            };
            unsafe { std::mem::replace(&mut (*object.get()).func.m_module, pointer) }
        };
        drop(RetiredOwnedCFields::one(old));
        Some(true)
    }
}

// Managed-view C-reference linearization, publication retirement, and view removal.
impl ObjectBridge {
    /// A managed immortal C view retains its stable runtime hold as a permanent
    /// external root. GC membership admission must preserve that lifetime after
    /// container mutation, without inspecting C layout from the runtime.
    pub fn is_immortal_c_view(&self, bits: AbiHandle) -> bool {
        let handle = self.handle_shard(bits).lock();
        handle.to_py.get(&bits).is_some_and(|entry| unsafe {
            crate::abi_types::is_immortal_refcnt((*entry.view.py_obj()).ob_refcnt)
        })
    }

    fn managed_entry_is_unique(&self, entry: &BridgeEntry, mirrored: usize) -> bool {
        if entry.lifecycle == BridgeLifecycle::FinalizingPin {
            return false;
        }
        let refs = unsafe { (*entry.view.py_obj()).ob_refcnt };
        let CReferenceCount::Counted(c_refs) =
            CReferenceCount::read(refs, "unique ownership query", entry.lifecycle).without_bias(
                entry.lifecycle.has_c_bias(),
                "unique ownership query",
                entry.lifecycle,
            )
        else {
            return false;
        };
        let runtime_owners = if MoltObject::from_bits(entry.bits).as_ptr().is_some() {
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(entry.bits) };
            if runtime_refs == molt_codegen_abi::IMMORTAL_REFCOUNT as usize {
                return false;
            }
            runtime_refs.checked_sub(1).unwrap_or_else(|| {
                abort_refcount_invariant("unique ownership runtime hold", refs, entry.lifecycle)
            })
        } else {
            0
        };
        let direct_refs = usize::try_from(c_refs)
            .ok()
            .and_then(|refs| refs.checked_sub(mirrored))
            .unwrap_or_else(|| {
                abort_refcount_invariant("unique ownership mirrors", refs, entry.lifecycle)
            });
        runtime_owners.checked_add(direct_refs) == Some(1)
    }

    /// Observe semantic ownership while excluding the bridge hold and duplicate
    /// private mirrors. A borrowed C bias does not imply unique runtime custody.
    pub unsafe fn managed_is_uniquely_referenced(&self, ptr: *mut PyObject) -> Option<bool> {
        let bits = self.managed_handle_for_pyobj(ptr)?;
        let (address, handle) = self.lock_address_then_handle(ptr.addr(), bits);
        let entry = handle.to_py.get(&bits)?;
        Some(
            self.managed_entry_is_unique(
                entry,
                address
                    .projection_refs
                    .get(&ptr.addr())
                    .copied()
                    .unwrap_or(0),
            ),
        )
    }

    /// Check and promote in one transaction. The stable runtime hold becomes
    /// the external lifetime root; no shared runtime header is immortalized.
    pub unsafe fn managed_try_set_immortal(&self, ptr: *mut PyObject) -> Option<bool> {
        let bits = self.managed_handle_for_pyobj(ptr)?;
        let (mut address, handle) = self.lock_address_then_handle(ptr.addr(), bits);
        let entry = handle.to_py.get(&bits)?;
        if !self.managed_entry_is_unique(
            entry,
            address
                .projection_refs
                .get(&ptr.addr())
                .copied()
                .unwrap_or(0),
        ) {
            return Some(false);
        }
        unsafe { (*ptr).ob_refcnt = crate::abi_types::IMMORTAL_REFCNT };
        address.projection_refs.remove(&ptr.addr());
        Some(true)
    }

    /// Canonical header mutation for Py_SET_REFCNT. Immortality is one-way;
    /// promotion absorbs all numeric private mirrors into the lifetime root.
    pub unsafe fn set_pyobj_refcnt(&self, ptr: *mut PyObject, refs: isize) {
        if ptr.is_null() {
            return;
        }
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        if let Some(bits) = self.managed_handle_for_pyobj(ptr) {
            let (mut address, handle) = self.lock_address_then_handle(ptr.addr(), bits);
            let entry = handle
                .to_py
                .get(&bits)
                .expect("owned refcount mutation lost its view");
            if unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
                return;
            }
            let ownership = CReferenceCount::read(refs, "Py_SET_REFCNT", entry.lifecycle);
            unsafe { (*ptr).ob_refcnt = refs };
            if matches!(ownership, CReferenceCount::Immortal) {
                address.projection_refs.remove(&ptr.addr());
            }
        } else {
            let mut address = self.address_shard(ptr.addr()).lock();
            if !unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
                unsafe { (*ptr).ob_refcnt = refs };
                if crate::abi_types::is_immortal_refcnt(refs) {
                    address.projection_refs.remove(&ptr.addr());
                }
            }
        }
    }

    /// Increment a canonical managed view while holding both identity ranks.
    /// The caller must hold the runtime execution token; the bridge lock makes
    /// the C header update linearizable with publication and retirement.
    pub unsafe fn managed_incref_pyobj(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        let addr = ptr.addr();
        loop {
            let address = self.address_shard(addr).lock();
            let bits = address.from_py.get(&addr).copied()?;
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let entry = handle.to_py.get_mut(&bits)?;
            match &entry.publication {
                PublicationState::Ready => {}
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                    if !observe_publication(self, bits) {
                        // Py_INCREF cannot report a recoverable failure. Never
                        // let an escaped rollback skeleton gain a reference
                        // which retirement cannot honor.
                        abort_refcount_invariant(
                            "Py_INCREF of an aborting publication",
                            unsafe { (*ptr).ob_refcnt },
                            entry.lifecycle,
                        );
                    }
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                    continue;
                }
            }
            if entry.view.py_obj() != ptr {
                return None;
            }
            let refs = unsafe { (*ptr).ob_refcnt };
            if crate::abi_types::is_immortal_refcnt(refs) {
                return Some(bits);
            }
            let Some(incremented) = checked_c_ref_increment(refs) else {
                abort_refcount_invariant("managed Py_INCREF", refs, entry.lifecycle);
            };
            unsafe { (*ptr).ob_refcnt = incremented };
            return Some(bits);
        }
    }

    /// Decrement a canonical managed view as one identity/refcount/lifecycle
    /// transaction.  Heap terminal release remains owned by the runtime;
    /// immediate values retire synchronously because they have no finalizer.
    pub unsafe fn managed_decref_pyobj(&self, ptr: *mut PyObject) -> ManagedDecref {
        let addr = ptr.addr();
        loop {
            let mut address = self.address_shard(addr).lock();
            let Some(bits) = address.from_py.get(&addr).copied() else {
                return ManagedDecref::NotManaged;
            };
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return ManagedDecref::NotManaged;
            };
            match &entry.publication {
                PublicationState::Ready => {}
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                    continue;
                }
            }
            if entry.view.py_obj() != ptr {
                return ManagedDecref::NotManaged;
            }
            let refs = unsafe { (*ptr).ob_refcnt };
            if crate::abi_types::is_immortal_refcnt(refs) {
                return ManagedDecref::Immortal;
            }
            if refs <= 0 {
                return ManagedDecref::Alive;
            }
            let remaining = refs - 1;
            unsafe { (*ptr).ob_refcnt = remaining };
            if remaining != 0 {
                return ManagedDecref::Alive;
            }
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                unsafe { (*ptr).ob_refcnt = 1 };
                return ManagedDecref::Alive;
            }
            if MoltObject::from_bits(bits).as_ptr().is_none() {
                entry.publication = PublicationState::Retiring;
                address.from_py.remove(&addr);
                address.direct_molt_py.remove(&addr);
                let entry = handle
                    .to_py
                    .remove(&bits)
                    .expect("inline managed view disappeared during decref");
                self.publication_ready[index].notify_all();
                drop(handle);
                drop(address);
                release_bridge_entry(*entry);
                return ManagedDecref::RetiredInline;
            }
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
            if runtime_refs > 1 {
                unsafe { (*ptr).ob_refcnt = 1 };
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
                return ManagedDecref::Alive;
            }
            // Publish the terminal handoff before returning to the runtime
            // header release. Checked non-owning upgrades lock this same entry
            // and must not slip between the C-zero verdict and that release.
            unsafe { (*ptr).ob_refcnt = 1 };
            entry.lifecycle = BridgeLifecycle::FinalizingPin;
            return ManagedDecref::ReleaseRuntimeHold(bits);
        }
    }

    pub fn release_pyobj(&self, ptr: *mut PyObject) -> PyObjRelease {
        if ptr.is_null() {
            return PyObjRelease::Untracked;
        }
        if pyobj_to_handle_static(ptr).is_some() {
            return PyObjRelease::StaticImmortal;
        }
        let addr = ptr.addr();
        loop {
            let mut address = self.address_shard(addr).lock();
            if address.numeric_carriers.contains_key(&addr) {
                return PyObjRelease::NumericCarrier;
            }
            let Some(bits) = address.from_py.get(&addr).copied() else {
                return PyObjRelease::Untracked;
            };
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            if handle.managed_address(bits) != Some(addr) {
                if address.direct_molt_py.get(&addr).copied() != Some(bits) {
                    return PyObjRelease::Untracked;
                }
                // Direct ingress owns its forward pair independently of any
                // reverse entry. A noncanonical alias may have no reverse, or
                // may share a handle whose canonical projection is managed.
                // Neither case grants authority to retire that other view.
                if handle
                    .raw_py
                    .get(&bits)
                    .is_some_and(|binding| binding.address == addr)
                {
                    handle.raw_py.remove(&bits);
                }
                address.direct_molt_py.remove(&addr);
                address.from_py.remove(&addr);
                return PyObjRelease::DirectViewUnregistered;
            }
            let entry = handle
                .to_py
                .get_mut(&bits)
                .expect("exact managed address lost its locked owner");
            match &entry.publication {
                PublicationState::Ready => entry.publication = PublicationState::Retiring,
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                    entry.publication = PublicationState::Retiring;
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                    continue;
                }
            }
            address.direct_molt_py.remove(&addr);
            address.from_py.remove(&addr);
            let entry = handle
                .to_py
                .remove(&bits)
                .expect("managed publication disappeared during retirement");
            self.publication_ready[index].notify_all();
            drop(handle);
            drop(address);
            release_bridge_entry(*entry);
            unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
            return PyObjRelease::ManagedViewRetired;
        }
    }

    fn remove_managed_view(&self, bits: AbiHandle, addr: usize) -> bool {
        let index = self.handle_shard_index(bits);
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        if let Some(entry) = handle.to_py.get_mut(&bits) {
            entry.publication = PublicationState::Retiring;
        }
        let entry = handle.to_py.remove(&bits);
        self.publication_ready[index].notify_all();
        drop(handle);
        drop(address);
        if let Some(entry) = entry {
            release_bridge_entry(*entry);
            true
        } else {
            false
        }
    }

    fn retire_managed_view_entry_deferred(&self, bits: AbiHandle) -> Option<Box<BridgeEntry>> {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        if entry.view.py_obj().addr() != addr {
            return None;
        }
        // This is ordinary RC terminal removal, not forced interpreter
        // retirement. Physical C owners must have drained before rc-zero can
        // revoke identity. Runtime/collector pins never license skipping them.
        assert_eq!(
            address.projection_refs.get(&addr).copied().unwrap_or(0),
            0,
            "terminal view removal preceded incoming projection release"
        );
        entry.publication = PublicationState::Retiring;
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        let entry = handle.to_py.remove(&bits)?;
        self.publication_ready[self.handle_shard_index(bits)].notify_all();
        drop(handle);
        drop(address);
        Some(entry)
    }

    /// Ordinary rc-zero retirement after direct C owners and incoming mirrors
    /// have drained. This includes TYPE_ID_TYPE; it cannot force a live Type
    /// out of an owning projection cycle. Interpreter cohort retirement uses
    /// retire_runtime_type_views; GC detaches mirrors and preserves identity.
    /// Physical outgoing edges wait until semantic sources are empty.
    pub fn retire_runtime_object_deferred(&self, bits: AbiHandle) -> Option<RetiredRuntimeView> {
        let entry = self.retire_managed_view_entry_deferred(bits)?;
        Some(RetiredRuntimeView { entry: Some(entry) })
    }

    /// Observe physical view presence without materializing an identity.
    pub fn has_managed_type_view(&self, bits: AbiHandle) -> bool {
        self.handle_shard(bits)
            .lock()
            .to_py
            .get(&bits)
            .is_some_and(|entry| matches!(entry.view, ManagedView::Type { .. }))
    }

    /// Retire the complete incoming projection graph of runtime type
    /// views. A type's MRO owns a C edge back to the type; any retained tuple,
    /// list or other concrete view can in turn own that MRO/type view. Runtime
    /// pins alone cannot keep those physical C allocations alive.
    ///
    /// The caller owns shutdown callback custody and pins the canonical class
    /// roots. Both runtime/C thread-state domains and native owners must have
    /// drained first: no saved exception or external guard may later restore
    /// or decref a selected pointer. This belongs in the outer callback fixed
    /// point before semantic class metadata is detached; releasing an alias's
    /// stable hold can run Python and requires another owner-drain pass.
    /// Existing publication transaction custody supplies the C pins, closed
    /// observations, edge drain, identity revocation and final runtime releases.
    /// No independent address registry or raw post-revocation decref is used.
    pub fn retire_runtime_type_views(&self, roots: &[AbiHandle]) -> bool {
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        let roots: std::collections::HashSet<_> = roots
            .iter()
            .copied()
            .filter(|&bits| self.has_managed_type_view(bits))
            .collect();
        if roots.is_empty() {
            return false;
        }
        struct Node {
            bits: AbiHandle,
            pointer: *mut PyObject,
            selected: bool,
        }
        let mut nodes = Vec::new();
        let mut incoming: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        let mut pending = Vec::new();
        for shard in self.handle_shards.iter() {
            let mut shard = shard.lock();
            for (&bits, entry) in &mut shard.to_py {
                let selected = roots.contains(&bits);
                let pointer = entry.view.py_obj();
                let owner = nodes.len();
                entry.view.owned_items_with(false, false, |edge, _| {
                    if !edge.is_null() {
                        incoming.entry(edge.addr()).or_default().push(owner);
                    }
                });
                if selected {
                    pending.push(owner);
                }
                nodes.push(Node {
                    bits,
                    pointer,
                    selected,
                });
            }
        }
        // Reverse closure includes every physical owner, including ordinary
        // C-writable fields as well as mirrors. Following only type -> MRO
        // would miss a retained alias or a sibling's bases/MRO projection.
        // One indexed worklist visits each node/edge once; no per-class scan.
        let mut cursor = 0;
        while cursor < pending.len() {
            let address = nodes[pending[cursor]].pointer.addr();
            cursor += 1;
            if let Some(owners) = incoming.get(&address) {
                for &owner in owners {
                    if !nodes[owner].selected {
                        nodes[owner].selected = true;
                        pending.push(owner);
                    }
                }
            }
        }
        drop(incoming);
        drop(pending);
        let component: Vec<_> = nodes.into_iter().filter(|node| node.selected).collect();
        // Reserve the complete transaction before changing an identity or
        // acquiring a pin. The GIL excludes mutation throughout the snapshot.
        let drain_now = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let drain_now =
                stack.frames.is_empty() && stack.active.is_empty() && !stack.rolling_back;
            stack.frames.reserve(component.len());
            drain_now
        });
        for node in component {
            let mut handle = self.handle_shard(node.bits).lock();
            let entry = handle
                .to_py
                .get_mut(&node.bits)
                .expect("retiring view disappeared");
            assert_eq!(entry.view.py_obj(), node.pointer);
            assert_eq!(entry.publication, PublicationState::Ready);
            let refs = unsafe { (*node.pointer).ob_refcnt };
            if !crate::abi_types::is_immortal_refcnt(refs) {
                let pinned = checked_c_ref_increment(refs).unwrap_or_else(|| {
                    abort_refcount_invariant(
                        "runtime projection retirement pin",
                        refs,
                        entry.lifecycle,
                    )
                });
                unsafe { (*node.pointer).ob_refcnt = pinned };
            }
            // Adopt the live view into the same closed transaction state used
            // by failed recursive publication. Decrefs still resolve through
            // the canonical map; observe_publication rejects new observers.
            entry.publication = PublicationState::Building {
                owner: std::thread::current().id(),
            };
            PUBLICATION_BUILD_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                let dependency = stack.frames.len();
                stack.frames.push(PublicationFrame {
                    bridge: self,
                    bits: node.bits,
                    pointer: node.pointer,
                    dependency,
                    complete: false,
                    failed: true,
                    committed: false,
                    pin_released: false,
                    aborting: true,
                    edges_cleared: false,
                    retired: None,
                });
            });
        }
        // Reentrant retirement can retire a disjoint component while a
        // publication/retirement already owns this stack. Its existing outer
        // transaction drains the appended closed frames; their pins keep every
        // allocation alive in the meantime.
        if drain_now {
            crate::api::errors::with_preserved_error(|| self.finish_publication_stack());
        }
        true
    }
    /// Add one runtime owner while holding the canonical ABI-view lifecycle
    /// lock. The supplied header transition therefore cannot race the mirrored
    /// `RuntimeOwned <-> ViewHoldOnly` bias or the `FinalizingPin` terminal gate.
    /// Non-owning callers pass `allow_finalizing = false`; ordinary owned
    /// resurrection inside Python finalizer code passes `true`.
    pub fn transition_runtime_owner_add<F>(
        &self,
        bits: AbiHandle,
        allow_finalizing: bool,
        internal_pins: u32,
        retain: F,
    ) -> Option<u32>
    where
        F: FnOnce() -> Option<u32>,
    {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (_address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        if entry.lifecycle == BridgeLifecycle::FinalizingPin && !allow_finalizing {
            return None;
        }
        let previous = retain()?;
        let semantic_previous = previous.checked_sub(internal_pins).unwrap_or_else(|| {
            eprintln!(
                "molt fatal: ABI runtime-owner add underflow previous={previous} internal_pins={internal_pins}"
            );
            std::process::abort();
        });
        match (semantic_previous, entry.lifecycle) {
            (1, BridgeLifecycle::ViewHoldOnly) => {
                entry.change_c_bias(true, "runtime owner add");
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
            }
            (_, BridgeLifecycle::ViewHoldOnly) => {
                eprintln!(
                    "molt fatal: view-hold-only ABI lifecycle had multiple runtime owners before add"
                );
                std::process::abort();
            }
            (_, BridgeLifecycle::RuntimeOwned | BridgeLifecycle::FinalizingPin) => {}
        }
        Some(previous)
    }

    /// Release one runtime owner as the inverse transaction. A terminal result
    /// publishes `FinalizingPin` before either bridge lock is released; checked
    /// weakref retention consequently rejects the stable view baseline before
    /// the runtime exposes its internal revival pin.
    pub fn transition_runtime_owner_release<F, R>(
        &self,
        bits: AbiHandle,
        internal_pins: u32,
        release: F,
        restore_stable_view_hold: R,
    ) -> Option<RuntimeOwnerRelease>
    where
        F: FnOnce() -> u32,
        R: FnOnce(),
    {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (_address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        let previous = release();
        let semantic_previous = previous.checked_sub(internal_pins).unwrap_or_else(|| {
            eprintln!(
                "molt fatal: ABI runtime-owner release underflow previous={previous} internal_pins={internal_pins}"
            );
            std::process::abort();
        });
        if semantic_previous > 2 {
            if entry.lifecycle == BridgeLifecycle::ViewHoldOnly {
                eprintln!("molt fatal: view-hold-only ABI lifecycle retained multiple owners");
                std::process::abort();
            }
            return Some(RuntimeOwnerRelease {
                previous,
                should_finalize: false,
            });
        }
        let py_obj = entry.view.py_obj();
        if semantic_previous == 2 {
            match entry.lifecycle {
                BridgeLifecycle::FinalizingPin => {
                    return Some(RuntimeOwnerRelease {
                        previous,
                        should_finalize: false,
                    });
                }
                BridgeLifecycle::ViewHoldOnly => {
                    eprintln!(
                        "molt fatal: canonical ABI view lost runtime-owner bias before owner drop"
                    );
                    std::process::abort();
                }
                BridgeLifecycle::RuntimeOwned => {}
            }
        } else {
            if semantic_previous != 1 {
                eprintln!(
                    "molt fatal: invalid stable-view runtime release previous={previous} internal_pins={internal_pins} lifecycle={:?}",
                    entry.lifecycle
                );
                std::process::abort();
            }
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                restore_stable_view_hold();
                return Some(RuntimeOwnerRelease {
                    previous,
                    should_finalize: true,
                });
            }
        }
        if entry.lifecycle == BridgeLifecycle::RuntimeOwned {
            entry.change_c_bias(false, "runtime owner drop");
            entry.lifecycle = BridgeLifecycle::ViewHoldOnly;
        }
        if semantic_previous == 1 {
            restore_stable_view_hold();
        }
        if unsafe { (*py_obj).ob_refcnt } != 0 {
            return Some(RuntimeOwnerRelease {
                previous,
                should_finalize: false,
            });
        }
        unsafe { (*py_obj).ob_refcnt = 1 };
        entry.lifecycle = BridgeLifecycle::FinalizingPin;
        Some(RuntimeOwnerRelease {
            previous,
            should_finalize: true,
        })
    }

    /// Rebase an immortal managed view onto the ordinary runtime-owner bias
    /// before its immortal runtime object enters the shutdown-only mortal lane.
    ///
    /// CPython reference operations on an immortal view are deliberately
    /// no-ops, so there are no countable direct C owners to preserve at this
    /// boundary. Runtime shutdown has already stopped new execution entrants;
    /// it may therefore restore the single encoded owner bias that the normal
    /// `RuntimeOwned -> ViewHoldOnly` terminal transition consumes.
    pub fn prepare_runtime_immortal_for_shutdown(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        let py_obj = entry.view.py_obj();
        if !unsafe { crate::abi_types::is_immortal_refcnt((*py_obj).ob_refcnt) } {
            return;
        }
        if entry.lifecycle != BridgeLifecycle::RuntimeOwned {
            eprintln!(
                "molt fatal: immortal canonical ABI view entered shutdown outside RuntimeOwned: {:?}",
                entry.lifecycle
            );
            std::process::abort();
        }
        unsafe { (*py_obj).ob_refcnt = 1 };
    }

    /// Direct C references created during a finalizer/weakref revival window
    /// are resurrection roots even though they do not change runtime RC.
    pub fn has_direct_c_refs(&self, bits: AbiHandle) -> bool {
        let (ptr, refs, lifecycle) = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return false;
            };
            (
                entry.view.py_obj(),
                unsafe { (*entry.view.py_obj()).ob_refcnt },
                entry.lifecycle,
            )
        };
        let CReferenceCount::Counted(direct_and_projection) =
            CReferenceCount::read(refs, "direct C reference query", lifecycle).without_bias(
                lifecycle.has_c_bias(),
                "direct C reference query",
                lifecycle,
            )
        else {
            return true;
        };
        let projection = self.mirrored_c_refcount(ptr.addr());
        let Ok(projection) = isize::try_from(projection) else {
            abort_refcount_invariant("projection reference query", refs, lifecycle);
        };
        if projection > direct_and_projection {
            abort_refcount_invariant("projection reference query", refs, lifecycle);
        }
        direct_and_projection != projection
    }

    pub fn begin_finalization(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        if entry.lifecycle == BridgeLifecycle::FinalizingPin {
            if unsafe { (*entry.view.py_obj()).ob_refcnt } < 1 {
                eprintln!("molt fatal: canonical ABI view lost its published finalizing pin");
                std::process::abort();
            }
            return;
        }
        if entry.lifecycle != BridgeLifecycle::ViewHoldOnly {
            eprintln!(
                "molt fatal: canonical ABI view finalization began outside ViewHoldOnly: {:?}",
                entry.lifecycle
            );
            std::process::abort();
        }
        let py_obj = entry.view.py_obj();
        if unsafe { (*py_obj).ob_refcnt } != 0 {
            eprintln!("molt fatal: finalization began with unmatched direct C references");
            std::process::abort();
        }
        unsafe { (*py_obj).ob_refcnt = 1 };
        entry.lifecycle = BridgeLifecycle::FinalizingPin;
    }

    /// Adopt a canonical view first published by arbitrary finalizer code into
    /// the already-open runtime revival window. The publication's runtime-owner
    /// bias becomes the finalization pin in place: the stable runtime view hold
    /// remains part of the runtime baseline and any additional direct C roots
    /// remain visible above this pin.
    pub fn begin_finalization_for_new_view(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            eprintln!("molt fatal: finalizer-published ABI view disappeared");
            std::process::abort();
        };
        if entry.lifecycle != BridgeLifecycle::RuntimeOwned {
            eprintln!(
                "molt fatal: finalizer-published ABI view was not runtime-owned: {:?}",
                entry.lifecycle
            );
            std::process::abort();
        }
        if unsafe { (*entry.view.py_obj()).ob_refcnt } < 1 {
            eprintln!("molt fatal: finalizer-published ABI view lost its runtime bias");
            std::process::abort();
        }
        entry.lifecycle = BridgeLifecycle::FinalizingPin;
    }

    pub fn finish_finalization(&self, bits: AbiHandle, runtime_resurrected: bool) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        if entry.lifecycle != BridgeLifecycle::FinalizingPin {
            eprintln!("molt fatal: canonical ABI view finalization state was lost");
            std::process::abort();
        }
        if runtime_resurrected {
            entry.lifecycle = BridgeLifecycle::RuntimeOwned;
        } else {
            entry.change_c_bias(false, "finalization pin release");
            entry.lifecycle = BridgeLifecycle::ViewHoldOnly;
        }
    }

    /// Retire a canonical view only after the runtime has passed every
    /// finalizer/weakref resurrection check and is committed to freeing the
    /// object. This preserves pointer identity throughout the revival window.
    pub fn runtime_object_destroyed(&self, bits: AbiHandle) {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return;
            };
            entry.view.py_obj().addr()
        };
        self.remove_managed_view(bits, addr);
    }
}

// Finalization completion, resurrection, GC roots, and zero-ref disposition.
impl ObjectBridge {
    /// Remove the stable view hold and private runtime-mirrored C references
    /// from the cycle collector's scratch count. C-writable physical fields
    /// contribute ordinary C references; their owners independently traverse
    /// their current pointers, including uncommitted direct writes.
    pub fn gc_ref_adjustment(&self, bits: AbiHandle) -> isize {
        let (ptr, c_refs, lifecycle) = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return 0;
            };
            (
                entry.view.py_obj(),
                unsafe { (*entry.view.py_obj()).ob_refcnt },
                entry.lifecycle,
            )
        };
        let CReferenceCount::Counted(direct_and_projection) =
            CReferenceCount::read(c_refs, "GC external-root adjustment", lifecycle).without_bias(
                lifecycle.has_c_bias(),
                "GC external-root adjustment",
                lifecycle,
            )
        else {
            // The stable runtime hold is an external root for the process-long
            // C lifetime. Neither that hold nor old mirrors are discounted.
            return 0;
        };
        let projection = self.mirrored_c_refcount(ptr.addr());
        let Ok(projection) = isize::try_from(projection) else {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        };
        let Some(c_owned_refs) = direct_and_projection.checked_sub(projection) else {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        };
        if c_owned_refs < 0 {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        }
        c_owned_refs - 1
    }

    /// Finalization is an explicit GC root until the runtime pin is resolved.
    /// This is intentionally queried by the collector instead of inferred from
    /// a transient refcount/bias arithmetic coincidence.
    pub fn has_finalizing_pin(&self, bits: AbiHandle) -> bool {
        let handle = self.handle_shard(bits).lock();
        handle
            .to_py
            .get(&bits)
            .is_some_and(|entry| entry.lifecycle == BridgeLifecycle::FinalizingPin)
    }

    /// Handle direct CPython refcount reaching zero. Immediate scalar handles
    /// have no runtime allocation, finalizer, or resurrection window, so their
    /// canonical view is retired here. For heap handles, if other runtime
    /// owners remain, re-establish their borrowed-view bias. Otherwise keep the
    /// canonical view attached and tell `_Py_Dealloc` to release its runtime
    /// hold: the runtime terminal path owns finalization and retires identity
    /// only after the resurrection window closes.
    pub fn c_ref_zero(&self, bits: AbiHandle) -> CRefZero {
        if MoltObject::from_bits(bits).as_ptr().is_none() {
            let addr = {
                let handle = self.handle_shard(bits).lock();
                let Some(entry) = handle.to_py.get(&bits) else {
                    return CRefZero::ViewRetained;
                };
                entry.view.py_obj().addr()
            };
            self.remove_managed_view(bits, addr);
            return CRefZero::ViewRetained;
        }
        {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return CRefZero::ViewRetained;
            };
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
                return CRefZero::ViewRetained;
            }
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
            if runtime_refs > 1 {
                unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
                return CRefZero::ViewRetained;
            }
            unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
            entry.lifecycle = BridgeLifecycle::FinalizingPin;
        }
        CRefZero::ReleaseRuntimeHold
    }

    fn classify_handle(bits: AbiHandle) -> MoltTypeTag {
        let obj = MoltObject::from_bits(bits);
        if obj.is_none() {
            return MoltTypeTag::None;
        }
        if obj.is_bool() {
            return MoltTypeTag::Bool;
        }
        if obj.is_int() {
            return MoltTypeTag::Int;
        }
        if obj.is_float() {
            return MoltTypeTag::Float;
        }
        if obj.is_ptr() {
            let hooks = crate::hooks::hooks_or_stubs();
            let tag = unsafe { (hooks.classify_heap)(bits) };
            match tag {
                value if value == MoltTypeTag::BuiltinCallable as u8 => {
                    MoltTypeTag::BuiltinCallable
                }
                value if value == MoltTypeTag::Int as u8 => MoltTypeTag::Int,
                value if value == MoltTypeTag::Complex as u8 => MoltTypeTag::Complex,
                value if value == MoltTypeTag::Str as u8 => MoltTypeTag::Str,
                value if value == MoltTypeTag::Bytes as u8 => MoltTypeTag::Bytes,
                value if value == MoltTypeTag::MemoryView as u8 => MoltTypeTag::MemoryView,
                value if value == MoltTypeTag::Slice as u8 => MoltTypeTag::Slice,
                value if value == MoltTypeTag::List as u8 => MoltTypeTag::List,
                value if value == MoltTypeTag::Tuple as u8 => MoltTypeTag::Tuple,
                value if value == MoltTypeTag::Dict as u8 => MoltTypeTag::Dict,
                value if value == MoltTypeTag::Set as u8 => MoltTypeTag::Set,
                value if value == MoltTypeTag::FrozenSet as u8 => MoltTypeTag::FrozenSet,
                value if value == MoltTypeTag::Type as u8 => MoltTypeTag::Type,
                value if value == MoltTypeTag::Module as u8 => MoltTypeTag::Module,
                value if value == MoltTypeTag::Traceback as u8 => MoltTypeTag::Traceback,
                value if value == MoltTypeTag::Exception as u8 => MoltTypeTag::Exception,
                _ => MoltTypeTag::Other,
            }
        } else {
            MoltTypeTag::Other
        }
    }
}

impl Default for ObjectBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the canonical managed ABI view to its runtime value identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_pyobj_to_handle(ptr: *mut PyObject) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    match GLOBAL_BRIDGE.observed_handle_for_pyobj(ptr) {
        Some(handle) => handle.bits(),
        None if matches!(resolve_pyobject(ptr), Some(ResolvedPyObject::Foreign)) => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"foreign PyObject has no managed Molt value identity".as_ptr(),
                )
            };
            0
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_pyobj_is_bridge_managed(ptr: *mut PyObject) -> i32 {
    GLOBAL_BRIDGE.pyobj_to_handle(ptr).is_some() as i32
}

/// Semantic type identity for source-recompiled extension consumers. The
/// runtime class edge owns identity; builtin class handles resolve to their
/// bound static types and user classes to their canonical managed Type views.
/// Physical `ob_type` remains an honest layout discriminator, not a substitute
/// for the runtime class of a managed heap value.
pub(crate) unsafe fn semantic_type(ptr: *mut PyObject) -> *mut PyTypeObject {
    let Some(resolved) = resolve_pyobject(ptr) else {
        return std::ptr::null_mut();
    };
    unsafe { semantic_type_for_resolved(ptr, resolved) }
}

/// The caller owns an admitted observation of this same live pointer. Reuse it
/// within that transaction; never retain it across a user callback/mutation.
pub(crate) unsafe fn semantic_type_for_resolved(
    ptr: *mut PyObject,
    resolved: ResolvedPyObject,
) -> *mut PyTypeObject {
    let ResolvedPyObject::ManagedMolt(handle) = resolved else {
        return unsafe { (*ptr).ob_type };
    };
    let value = handle.decode();
    if value.is_none() {
        return &raw mut crate::abi_types::PyNone_Type;
    }
    if value.is_bool() {
        return &raw mut crate::abi_types::PyBool_Type;
    }
    if value.is_int() {
        return &raw mut crate::abi_types::PyLong_Type;
    }
    if value.is_float() {
        return &raw mut crate::abi_types::PyFloat_Type;
    }
    unsafe { GLOBAL_BRIDGE.runtime_class_view(handle.bits()) }
}

/// Exact public type predicates use Python class identity, never the storage
/// tag of a bridge view. This shares Py_TYPE's live class projection and stays
/// correct after legal __class__ reassignment without another type cache.
pub(crate) unsafe fn is_exact_semantic_type(
    object: *mut PyObject,
    expected: *mut PyTypeObject,
) -> bool {
    !object.is_null()
        && !expected.is_null()
        && std::ptr::eq(unsafe { semantic_type(object) }, expected)
}

/// Inclusive public type predicates follow the same class authority as exact
/// predicates. Native storage tags describe representation, not inheritance.
pub(crate) unsafe fn is_semantic_instance_of(
    object: *mut PyObject,
    expected: *mut PyTypeObject,
) -> bool {
    if object.is_null() || expected.is_null() {
        return false;
    }
    let actual = unsafe { semantic_type(object) };
    !actual.is_null()
        && (std::ptr::eq(actual, expected)
            || unsafe { crate::api::typeobj::PyType_IsSubtype(actual, expected) } != 0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_semantic_type(ptr: *mut PyObject) -> *mut PyTypeObject {
    unsafe { semantic_type(ptr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_set_semantic_type(
    ptr: *mut PyObject,
    new_type: *mut PyTypeObject,
) -> i32 {
    if ptr.is_null() || new_type.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    if GLOBAL_BRIDGE.managed_handle_for_pyobj(ptr).is_some() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"cannot change the type of a managed Molt value".as_ptr(),
            );
        }
        return -1;
    }
    unsafe { (*ptr).ob_type = new_type };
    0
}

/// Materialize the one ABI `PyObject*` representation for a Molt handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_handle_to_pyobj(bits: u64) -> *mut PyObject {
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_handle_to_borrowed_pyobj(bits: u64) -> *mut PyObject {
    unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) }
}

/// Header writes use the same lifetime and mirrored-edge authority as refcounts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_set_refcnt(ptr: *mut PyObject, refs: isize) {
    unsafe { GLOBAL_BRIDGE.set_pyobj_refcnt(ptr, refs) };
}

/// Every owned runtime result crossing into C receives a stable physical
/// `PyObject` representation; runtime value bits are never exposed as pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_result_to_pyobj(bits: u64) -> *mut PyObject {
    if bits == 0 && unsafe { (crate::hooks::hooks_or_stubs().exception_pending)() } != 0 {
        return std::ptr::null_mut();
    }
    let obj = MoltObject::from_bits(bits);
    let scalar = obj.is_int()
        || obj.is_bool()
        || obj.is_float()
        || obj.is_ptr()
            && matches!(
                unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits) },
                tag if tag == crate::abi_types::MoltTypeTag::Int as u8
                    || tag == crate::abi_types::MoltTypeTag::Complex as u8
            );
    if scalar {
        let (ptr, owned) = unsafe { crate::api::numbers::materialize_numeric_owned_handle(bits) };
        if !ptr.is_null() {
            debug_assert!(
                owned || obj.is_bool() || crate::api::numbers::is_cached_small_int_handle(bits)
            );
            return ptr;
        }
        return std::ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_any_incref(ptr: *mut PyObject) {
    match resolve_pyobject(ptr) {
        Some(ResolvedPyObject::ManagedMolt(_)) => unsafe { crate::api::refcount::Py_INCREF(ptr) },
        Some(ResolvedPyObject::Foreign) => unsafe { crate::api::refcount::Py_INCREF(ptr) },
        None => {}
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_any_decref(ptr: *mut PyObject) {
    match resolve_pyobject(ptr) {
        Some(ResolvedPyObject::ManagedMolt(_)) => unsafe { crate::api::refcount::Py_DECREF(ptr) },
        Some(ResolvedPyObject::Foreign) => unsafe { crate::api::refcount::Py_DECREF(ptr) },
        None => {}
    }
}

/// Stateless `*mut PyObject` → Molt handle translation for static singletons.
///
/// Recognises `Py_None` / `Py_True` / `Py_False` directly.  Returns `None`
/// for non-singleton pointers; callers use the explicit bridge registries.
///
/// Pointer-equality only — no dereference — so this function is safe to
/// call with any `*mut PyObject` value (including dangling).
fn pyobj_to_handle_static(ptr: *mut PyObject) -> Option<AbiHandle> {
    if ptr.is_null() {
        return None;
    }
    if std::ptr::eq(ptr, &raw const Py_None as *const _) {
        return Some(MoltObject::none().bits());
    }
    if std::ptr::eq(ptr, &raw const Py_True as *const _) {
        return Some(MoltObject::from_bool(true).bits());
    }
    if std::ptr::eq(ptr, &raw const Py_False as *const _) {
        return Some(MoltObject::from_bool(false).bits());
    }
    if let Some(bits) = crate::api::numbers::cached_small_int_bits_from_ptr(ptr) {
        return Some(bits);
    }
    // None still has a legacy Rust-side storage name. Bool has no second lane:
    // `Py_True`/`Py_False` are Rust aliases of the canonical `_Py_*Struct`
    // storage already checked above.
    if std::ptr::eq(
        ptr,
        &raw const crate::api::object::_Py_NoneStruct as *const _,
    ) {
        return Some(MoltObject::none().bits());
    }
    None
}

// ─── Exported ABI initialiser ─────────────────────────────────────────────

/// Initialize the Molt CPython ABI bridge (type-tag table + static type objects).
///
/// Exposed as a `#[no_mangle]` C symbol so callers can `dlopen`
/// `libmolt_cpython_abi.dylib`, resolve this symbol, and call it before
/// loading any C extensions.  Idempotent — safe to call multiple times.
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        unsafe { crate::abi_types::initialize_static_type_storage() };
        unsafe { crate::api::typeobj::init_descriptor_slots() };
        // Give `PyType_Type` a `tp_getattro` that answers `type.__name__` /
        // `__qualname__` from `tp_name`, so metaclasses (numpy's `_DTypeMeta`)
        // inherit it and `DType.__name__` resolves once a DType crosses into
        // Molt as a foreign wrapper.
        unsafe { crate::api::typeobj::init_type_getattro() };
        init_tag_table();
        // Publish the `datetime.datetime_CAPI` capsule so a C extension's
        // `PyDateTime_IMPORT` (`PyCapsule_Import("datetime.datetime_CAPI", 0)`)
        // resolves the datetime C API — numpy's `_multiarray_umath` init does
        // this and returned NULL when the capsule was absent (silent-failure).
        crate::api::datetime::register_datetime_capi();
    });
}

// ─── Foreign-object custody: dispatch back through the C type slots ────────────
//
// A runtime `TYPE_ID_FOREIGN` wrapper stores a genuine C-extension `PyObject*`.
// When compiled Python performs `getattr` / `setattr` / a call on the wrapper,
// the runtime extracts the C pointer and calls one of these functions (a direct
// cross-crate Rust call — the ABI is statically linked into the runtime binary).
// Each routes through the wrapped object's OWN type slots and converts the
// C result back into an owned Molt value via `molt_value_for_pyobj`. They must
// NOT re-enter `PyObject_GetAttr`/`PyObject_SetAttr` (whose first branch is the
// bridge hook back into the runtime), or a foreign object would recurse forever;
// they go straight to `tp_getattro` / `tp_setattro` / `PyObject_Call`.

/// Whether a foreign C object can participate in CPython cyclic GC.
///
/// GC-capable objects must be enrolled in the runtime's native GC authority
/// before a foreign wrapper is published; the runtime checks that enrollment.
///
/// # Safety
/// `c_ptr` must identify a live `PyObject` while the GIL is held.
pub unsafe fn molt_foreign_object_is_gc_capable(c_ptr: usize) -> bool {
    if c_ptr == 0 {
        return false;
    }
    let object = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    let ty = unsafe { (*object).ob_type };
    if ty.is_null() {
        return false;
    }
    if let Some(is_gc) = unsafe { (*ty).tp_is_gc } {
        // The canonical type slot's public result follows HEAPTYPE. Internal
        // custody must follow the physical allocation instead: extensions can
        // temporarily set that flag on a compact static declaration, or clear
        // it on a real heap type. Neither changes the storage we may traverse.
        if std::ptr::fn_addr_eq(
            is_gc,
            crate::api::typeobj::molt_type_is_gc
                as unsafe extern "C" fn(*mut PyObject) -> std::os::raw::c_int,
        ) {
            return crate::api::typeobj::heap_type_storage(object.cast()).is_some();
        }
        return unsafe { is_gc(object) != 0 };
    }
    unsafe { ((*ty).tp_flags & crate::abi_types::Py_TPFLAGS_HAVE_GC) != 0 }
}

/// Release the bridge identity + strong reference a foreign wrapper held on the
/// C object at `c_ptr`. Called from the runtime's `TYPE_ID_FOREIGN` drop hook.
///
/// # Safety
/// `c_ptr` is the C pointer a now-dropping foreign wrapper held; the matching
/// `Py_INCREF` was taken in [`ObjectBridge::foreign_wrapper_for`].
pub unsafe fn molt_foreign_object_release(c_ptr: usize) {
    if c_ptr == 0 {
        return;
    }
    // Drop the identity mapping under the bridge lock, then release the strong
    // reference OUTSIDE the lock (Py_DECREF may run a C tp_dealloc that
    // re-enters the bridge).
    unsafe { GLOBAL_BRIDGE.release_foreign(c_ptr) };
    unsafe {
        crate::api::refcount::Py_DECREF(core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr))
    };
}

/// Hash a foreign wrapper by routing through the wrapped C object's own
/// `tp_hash` (CPython `PyObject_Hash`). A numpy DType CLASS (a foreign C type
/// whose metatype inherits `type.__hash__`) hashes by identity here; a
/// genuinely-unhashable foreign type raises `TypeError` inside the C slot and
/// `PyObject_Hash` returns -1 with the exception left pending, which the caller
/// propagates. Returns the CPython hash value, or -1 on error.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_hash(c_ptr: usize) -> isize {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return -1;
    }
    unsafe { crate::api::typeobj::PyObject_Hash(obj) }
}

/// Consume a native slot's result exactly once. Both the callback and the
/// projection must establish an error on failure; neither cleanup may erase it.
unsafe fn owned_native_result_to_runtime(result: *mut PyObject) -> crate::hooks::OwnedHandleResult {
    let result = unsafe { crate::api::errors::check_native_result(result, "native slot") };
    if result.is_null() {
        unsafe { ensure_result_error(c"native slot returned NULL without an exception") };
        return crate::hooks::OwnedHandleResult::error();
    }
    let bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(result) };
    if bits.is_none() {
        unsafe {
            ensure_result_error(c"native slot result could not be projected to a runtime value")
        };
    }
    unsafe { crate::api::errors::release_preserving_error(&[result]) };
    bits.map_or_else(
        crate::hooks::OwnedHandleResult::error,
        crate::hooks::OwnedHandleResult::ok,
    )
}

/// Own the two optional projections across native descriptor reentry. Missing
/// operands use NULL; the exact None handle still produces a Py_None pointer.
struct NativeDescriptorOperands([*mut PyObject; 2]);

impl NativeDescriptorOperands {
    unsafe fn capture(operands: [Option<u64>; 2]) -> Option<Self> {
        let mut captured = Self([std::ptr::null_mut(); 2]);
        for (slot, bits) in captured.0.iter_mut().zip(operands) {
            if let Some(bits) = bits {
                *slot = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits) };
                if slot.is_null() {
                    unsafe { ensure_result_error(c"native descriptor operand projection failed") };
                    return None;
                }
            }
        }
        Some(captured)
    }
}

impl Drop for NativeDescriptorOperands {
    fn drop(&mut self) {
        unsafe { crate::api::errors::release_preserving_error(&self.0) };
    }
}

/// Read physical descriptor data status without projecting its type or looking
/// up names in a synthetic runtime class. The caller owns the live C object.
pub unsafe fn molt_foreign_descriptor_is_data(c_ptr: usize) -> bool {
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::descriptor::is_data(core::ptr::with_exposed_provenance_mut(c_ptr))
            .unwrap_or(false)
    })
}

/// Pure inquiry used by the runtime's live descriptor-type probe.
pub unsafe fn molt_foreign_descriptor_has_get(c_ptr: usize) -> bool {
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::descriptor::has_get(core::ptr::with_exposed_provenance_mut(c_ptr))
            .unwrap_or(false)
    })
}

/// Execute the same descriptor get slot as native generic attribute access.
/// Missing means there is no get slot, never that the callback failed.
pub unsafe fn molt_foreign_descriptor_get(
    c_ptr: usize,
    instance: Option<u64>,
    owner: Option<u64>,
) -> crate::hooks::OwnedHandleResult {
    let descriptor = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    match unsafe { crate::api::descriptor::has_get(descriptor) } {
        Ok(true) => {}
        Ok(false) => return crate::hooks::OwnedHandleResult::missing(),
        Err(()) => return crate::hooks::OwnedHandleResult::error(),
    }
    let Some(operands) = (unsafe { NativeDescriptorOperands::capture([instance, owner]) }) else {
        return crate::hooks::OwnedHandleResult::error();
    };
    match unsafe { crate::api::descriptor::get(descriptor, operands.0[0], operands.0[1]) } {
        Some(result) => unsafe { owned_native_result_to_runtime(result) },
        None => crate::hooks::OwnedHandleResult::missing(),
    }
}

/// Native set and delete share tp_descr_set. Absence is separate from an
/// explicit Python None assignment and from a failed native callback.
pub unsafe fn molt_foreign_descriptor_set(
    c_ptr: usize,
    instance: u64,
    value: Option<u64>,
) -> crate::hooks::OwnedHandleResult {
    let descriptor = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    match unsafe { crate::api::descriptor::is_data(descriptor) } {
        Ok(true) => {}
        Ok(false) => return crate::hooks::OwnedHandleResult::missing(),
        Err(()) => return crate::hooks::OwnedHandleResult::error(),
    }
    let Some(operands) = (unsafe { NativeDescriptorOperands::capture([Some(instance), value]) })
    else {
        return crate::hooks::OwnedHandleResult::error();
    };
    match unsafe { crate::api::descriptor::set(descriptor, operands.0[0], operands.0[1]) } {
        Some(status) if status >= 0 => {
            crate::hooks::OwnedHandleResult::ok(MoltObject::none().bits())
        }
        Some(_) => crate::hooks::OwnedHandleResult::error(),
        None => crate::hooks::OwnedHandleResult::missing(),
    }
}

/// `getattr` on a foreign wrapper: route through the wrapped C object's own
/// `tp_getattro` (else CPython generic getattr). `name_bits` is a Molt string
/// handle. The status is separate from the owned value, preserving float +0.0.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_getattr(
    c_ptr: usize,
    name_bits: u64,
) -> crate::hooks::OwnedHandleResult {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return crate::hooks::OwnedHandleResult::error();
    }
    let name_obj = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name_bits) };
    if name_obj.is_null() {
        return crate::hooks::OwnedHandleResult::error();
    }
    let result = unsafe { crate::api::object::native_get_attr(obj, name_obj) };
    unsafe { crate::api::errors::release_preserving_error(&[name_obj]) };
    unsafe { owned_native_result_to_runtime(result) }
}

/// Return the wrapped C object's type name (`tp_name`, a static C string) for
/// honest diagnostics of a foreign wrapper. Returns NULL when unavailable.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_type_name(c_ptr: usize) -> *const std::os::raw::c_char {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return std::ptr::null();
    }
    let tp = unsafe { (*obj).ob_type };
    if tp.is_null() {
        return std::ptr::null();
    }
    unsafe { (*tp).tp_name }
}

/// `setattr` on a foreign wrapper: route through the wrapped C object's own
/// `tp_setattro`. `None` means delete; every boxed value (including float zero)
/// is a valid assignment. Runtime arguments remain borrowed. Returns 0 on
/// success, -1 with the C slot's exception left pending on failure.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_setattr(
    c_ptr: usize,
    name_bits: u64,
    value_bits: Option<u64>,
) -> std::os::raw::c_int {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return -1;
    }
    let name_obj = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name_bits) };
    if name_obj.is_null() {
        return -1;
    }
    let value_obj = if let Some(bits) = value_bits {
        let value = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits) };
        if value.is_null() {
            unsafe { crate::api::errors::release_preserving_error(&[name_obj]) };
            return -1;
        }
        value
    } else {
        std::ptr::null_mut()
    };
    // Share public C-API slot dispatch, including the legacy setter and
    // slot-less TypeError. Keep temporary owners until result validation ends.
    let rc = unsafe { crate::api::object::PyObject_SetAttr(obj, name_obj, value_obj) };
    let rc = unsafe { crate::api::errors::check_native_status(rc, "native attribute mutation") };
    unsafe { crate::api::errors::release_preserving_error(&[name_obj, value_obj]) };
    rc
}

/// Call a foreign wrapper through the wrapped C object's `tp_call`. Exact Molt
/// tuples already own one canonical packed C projection, so the call borrows
/// that identity directly instead of allocating and copying a second tuple.
/// `args_bits` is a Molt tuple handle (0 = no positional args); `kwargs_bits` a
/// Molt dict handle (0 = none). Returns a typed owned result with an independent
/// status, so a successful floating-point zero is not a failure sentinel.
///
/// # Safety
/// `c_ptr` must be a live callable C-extension `PyObject*`.
pub unsafe fn molt_foreign_is_callable(c_ptr: usize) -> bool {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return false;
    }
    let tp = unsafe { (*obj).ob_type };
    !tp.is_null() && unsafe { (*tp).tp_call.is_some() }
}

/// # Safety
/// `c_ptr` must be a live callable C-extension `PyObject*`.
pub unsafe fn molt_foreign_call(
    c_ptr: usize,
    args_bits: u64,
    kwargs_bits: u64,
) -> crate::hooks::OwnedHandleResult {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return crate::hooks::OwnedHandleResult::error();
    }
    let args_obj = unsafe { tuple_view_from_molt(args_bits) };
    if args_obj.is_null() {
        return crate::hooks::OwnedHandleResult::error();
    }
    let kwargs_obj = if kwargs_bits == 0 {
        std::ptr::null_mut()
    } else {
        let kwargs = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(kwargs_bits) };
        if kwargs.is_null() {
            unsafe { crate::api::errors::release_preserving_error(&[args_obj]) };
            return crate::hooks::OwnedHandleResult::error();
        }
        kwargs
    };
    let result = unsafe { crate::api::object::PyObject_Call(obj, args_obj, kwargs_obj) };
    unsafe { crate::api::errors::release_preserving_error(&[args_obj, kwargs_obj]) };
    unsafe { owned_native_result_to_runtime(result) }
}

/// Acquire a new C reference to the canonical packed projection of a runtime
/// tuple without consuming the borrowed call argument.
unsafe fn tuple_view_from_molt(args_bits: u64) -> *mut PyObject {
    if args_bits == 0 {
        return unsafe { crate::api::sequences::PyTuple_New(0) };
    }
    unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(args_bits) }
}

/// Stable-ABI spelling of Py_XINCREF, routed through the canonical physical
/// PyObject refcount authority rather than reinterpreting the address as bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IncRef(obj: *mut PyObject) {
    unsafe { crate::api::refcount::Py_XINCREF(obj) };
}

/// Stable-ABI spelling of Py_XDECREF.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_DecRef(obj: *mut PyObject) {
    unsafe { crate::api::refcount::Py_XDECREF(obj) };
}

/// Compute the runtime hash of a Molt value directly from its NaN-boxed bits.
/// The C-ABI `PyObject_Hash` resolves a canonical bridge handle before calling
/// this helper. Never returns the raw `-1` error sentinel for a real value: an
/// integer that hashes to `-1` is remapped to `-2`, matching CPython.
pub(crate) fn molt_hash_from_bits(bits: u64) -> isize {
    let mo = MoltObject::from_bits(bits);

    if mo.is_ptr() {
        let hash = unsafe { (crate::hooks::hooks_or_stubs().object_hash)(bits) } as isize;
        if hash == -1 && unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"runtime hash authority unavailable".as_ptr(),
                );
            }
        }
        return hash;
    }

    let h = if mo.is_int() {
        // An inline Molt integer can exceed a 32-bit Py_hash_t/modulus.
        molt_lang_obj_model::hash_policy::hash_int(mo.as_int().unwrap_or(0)) as isize
    } else if mo.is_float() {
        let f = mo.as_float().unwrap_or(0.0);
        unsafe { crate::api::numbers::_Py_HashDouble(std::ptr::null_mut(), f) }
    } else if mo.is_bool() {
        mo.as_bool().unwrap_or(false) as isize
    } else if mo.is_none() {
        molt_lang_obj_model::hash_policy::normalize_hash(
            molt_lang_obj_model::hash_policy::PY_HASH_NONE,
        ) as isize
    } else {
        molt_lang_obj_model::hash_policy::normalize_hash(bits as i64) as isize
    };
    molt_lang_obj_model::hash_policy::normalize_hash(h as i64) as isize
}

#[cfg(test)]
mod bridge_handle_tests {
    use super::*;
    use crate::abi_types::PyUnicode_Type;

    #[test]
    fn immediate_hash_bridge_preserves_target_numeric_modulus() {
        // All inputs fit the inline object representation, but several exceed
        // the 32-bit ABI hash range. They must reduce numerically, not truncate.
        for (value, expected32) in [
            (-1i64, -2i64),
            (-2147483648, -2),
            (2147483647, 0),
            (2147483648, 1),
            (1099511627776, 512),
        ] {
            let expected = if crate::abi_types::Py_hash_t::BITS == 32 {
                expected32
            } else if value == -1 {
                -2
            } else {
                value
            };
            assert_eq!(
                molt_hash_from_bits(MoltObject::from_int(value).bits()) as i64,
                expected,
            );
        }
        for (value, expected32, expected64) in [
            (1.5, 1073741825, 1152921504606846977),
            (f64::from_bits(1), 2048, 16777216),
        ] {
            let expected = if crate::abi_types::Py_hash_t::BITS == 32 {
                expected32
            } else {
                expected64
            };
            let hash = unsafe { crate::api::numbers::_Py_HashDouble(std::ptr::null_mut(), value) };
            assert_eq!(hash as i64, expected);
        }
    }

    #[test]
    fn lifecycle_refcount_arithmetic_rejects_corrupt_boundaries() {
        assert_eq!(checked_c_refs_without_bias(0, false), Some(0));
        assert_eq!(checked_c_refs_without_bias(1, true), Some(0));
        assert_eq!(checked_c_refs_without_bias(4, true), Some(3));
        assert_eq!(checked_c_refs_without_bias(0, true), None);
        assert_eq!(checked_c_refs_without_bias(-1, false), None);
        assert_eq!(checked_c_ref_increment(-1), None);
        assert_eq!(checked_c_ref_increment(isize::MAX), None);
        assert_eq!(checked_c_ref_increment(3), Some(4));
    }

    #[test]
    fn reentrant_handle_stack_keeps_common_depth_inline() {
        let mut stack = ReentrantHandleStack::new();
        assert_eq!(stack.overflow.capacity(), 0);
        for bits in 1..=REENTRANT_HANDLE_INLINE_DEPTH as u64 {
            stack.push(bits);
        }
        assert_eq!(stack.depth, REENTRANT_HANDLE_INLINE_DEPTH);
        assert_eq!(stack.overflow.capacity(), 0);
        stack.push((REENTRANT_HANDLE_INLINE_DEPTH + 1) as u64);
        assert!(stack.overflow.capacity() > 0);
        for expected in (1..=(REENTRANT_HANDLE_INLINE_DEPTH + 1) as u64).rev() {
            assert_eq!(stack.pop(), Some(expected));
        }
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn tuple_view_uses_one_exact_c_layout_allocation() {
        let len = 17;
        let allocation =
            TupleAllocation::new(1, std::ptr::null_mut(), len).expect("tuple sidecar allocation");
        let item_offset = std::mem::offset_of!(crate::abi_types::PyTupleObject, ob_item);
        let expected_size = item_offset + len * std::mem::size_of::<*mut PyObject>();
        assert_eq!(
            allocation.layout.size(),
            expected_size,
            "the CPython prefix and exact inline item vector share one allocation"
        );
        assert_eq!(
            unsafe { (*allocation.object.as_ptr()).ob_base.ob_size },
            len as crate::abi_types::Py_ssize_t
        );
        assert_eq!(
            allocation.items_ptr().addr(),
            allocation.object.as_ptr().addr() + item_offset
        );
        assert!(allocation.items().iter().all(|pointer| pointer.is_null()));
        for refs in [1, 2, crate::abi_types::IMMORTAL_REFCNT] {
            let empty = TupleAllocation::new(refs, std::ptr::null_mut(), 0)
                .expect("empty tuple projection allocation");
            assert_eq!(unsafe { (*empty.py_obj()).ob_refcnt }, refs);
        }
    }

    /// Regression for the numpy `_multiarray_umath` "'str' object is not
    /// callable" frontier: `PyObject_Call` routes bridge-managed Molt callables
    /// to the runtime `object_call` hook via `molt_handle_for_pyobj`, which must
    /// return genuine Molt handles for minted proxies and MUST NOT hand a
    /// raw-registry synthetic handle (not valid `MoltObject` bits) to the
    /// runtime.
    #[test]
    fn molt_handle_for_pyobj_excludes_raw_registered_pointers() {
        let _thread = crate::api::object::AbiTestThreadStateTransaction::new();
        init_tag_table();
        let bridge = &*GLOBAL_BRIDGE;
        // Minted proxy for a genuine Molt handle resolves through both paths.
        let int_bits = MoltObject::from_int(0x5EED).bits();
        let proxy = unsafe { bridge.owned_handle_to_pyobj(int_bits) };
        assert_eq!(
            bridge.pyobj_to_handle(proxy).map(BridgeIdentity::as_handle),
            Some(int_bits)
        );
        assert_eq!(
            bridge
                .molt_handle_for_pyobj(proxy)
                .map(MoltValueHandle::bits),
            Some(int_bits)
        );
        // Without a runtime foreign-object hook, an arbitrary C object remains
        // unregistered; no synthetic non-Molt identity is fabricated.
        let mut stray = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let stray_ptr = &raw mut stray;
        assert_eq!(unsafe { bridge.molt_value_for_pyobj(stray_ptr) }, None);
        assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
        assert!(bridge.pyobj_to_handle(stray_ptr).is_none());
        assert_eq!(bridge.molt_handle_for_pyobj(stray_ptr), None);
        unsafe {
            crate::api::errors::PyErr_Clear();
            crate::api::refcount::Py_DECREF(proxy);
        }
    }

    /// `Other`-tagged Molt objects (compiled functions, classes, arbitrary
    /// instances) use the honest generic managed type and never masquerade as
    /// `str` or another concrete builtin.
    #[test]
    fn other_tag_maps_to_generic_managed_type_not_str() {
        init_tag_table();
        let ty = unsafe { tag_to_type(MoltTypeTag::Other) };
        assert!(
            std::ptr::eq(ty.cast_const(), &raw const MoltManaged_Type),
            "Other tag must map to MoltManaged_Type"
        );
        assert!(
            !std::ptr::eq(ty.cast_const(), &raw const PyUnicode_Type),
            "Other tag must not masquerade as str"
        );
    }

    /// `molt_value_for_pyobj` resolves static singletons and genuine Molt
    /// proxies to their canonical Molt handles WITHOUT foreign-wrapping them —
    /// only genuine C-extension objects get a `TYPE_ID_FOREIGN` wrapper.
    #[test]
    fn molt_value_for_pyobj_resolves_singletons_and_proxies() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        // Static singleton `None` → canonical NaN-boxed None, no wrapper.
        let none_ptr = &raw mut Py_None;
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(none_ptr) },
            Some(MoltObject::none().bits())
        );
        // A genuine Molt object that crossed to C (a bridge proxy) resolves back
        // to its own Molt handle, not a foreign wrapper.
        let int_bits = MoltObject::from_int(0x1234).bits();
        let proxy = unsafe { bridge.owned_handle_to_pyobj(int_bits) };
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(proxy) },
            Some(int_bits)
        );
        // A process-static shell can acquire successive runtime bindings;
        // canonical ingress must never create a foreign identity for either.
        let mut shell = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let shell_ptr = &raw mut shell;
        for value in [20_004, 20_005] {
            let bits = MoltObject::from_int(value).bits();
            assert!(
                unsafe { bridge.bind_static_pyobj_to_runtime_handle(shell_ptr, bits, true) }
                    .is_ok()
            );
            assert_eq!(
                unsafe { bridge.molt_value_for_pyobj(shell_ptr) },
                Some(bits)
            );
            assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(shell_ptr, bits) });
        }
        assert!(
            bridge
                .address_shards
                .iter()
                .all(|shard| shard.lock().foreign.is_empty())
        );
    }

    /// A foreign wrapper's identity round-trips: handed back to C it resolves to
    /// the ORIGINAL C pointer (via `raw_py`), and `release_foreign` drops both
    /// the `foreign` and `raw_py` identity entries.
    #[test]
    fn foreign_wrapper_round_trips_and_releases() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let mut fake = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let c_ptr = &raw mut fake;
        // Stand in for a minted `TYPE_ID_FOREIGN` wrapper handle (the runtime
        // hook is not linked in a pure-ABI test); install the identity entries
        // exactly as `foreign_wrapper_for` would.
        let w_bits = 0xBEEF_0000_0000_0010u64;
        // Expose once (the address is reconstructed into a pointer by
        // `handle_to_pyobj` below), then use it for the identity entries exactly
        // as `foreign_wrapper_for` does.
        let addr = c_ptr.expose_provenance();
        bridge.insert_foreign_for_test(c_ptr, w_bits);
        // The wrapper handed back to C resolves to the original C pointer.
        let back = unsafe { bridge.owned_handle_to_pyobj(w_bits) };
        assert_eq!(
            back, c_ptr,
            "foreign wrapper must round-trip to its C object"
        );
        // Release drops the identity mapping so a fresh wrapper can be minted.
        unsafe { bridge.release_foreign(addr) };
        assert!(
            !bridge
                .address_shard(addr)
                .lock()
                .foreign
                .contains_key(&addr)
        );
        assert!(
            !bridge
                .handle_shard(w_bits)
                .lock()
                .raw_py
                .contains_key(&w_bits)
        );
    }

    #[test]
    fn static_binding_unbind_retires_exact_canonical_pair() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let bits = 0xA110_0000_0000_0010;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, bits, true)
                .is_ok()
        });
        assert_eq!(
            bridge.address_shard(addr).lock().from_py.get(&addr),
            Some(&bits)
        );
        assert_eq!(
            bridge.address_shard(addr).lock().direct_molt_py.get(&addr),
            Some(&bits)
        );
        assert_eq!(
            bridge
                .handle_shard(bits)
                .lock()
                .raw_py
                .get(&bits)
                .map(|binding| &binding.address),
            Some(&addr)
        );

        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        let address = bridge.address_shard(addr).lock();
        assert!(!address.from_py.contains_key(&addr));
        assert!(!address.direct_molt_py.contains_key(&addr));
        drop(address);
        assert!(!bridge.handle_shard(bits).lock().raw_py.contains_key(&bits));
    }

    #[test]
    fn static_binding_unbind_accepts_noncanonical_alias_without_reverse() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let bits = 0xA110_0000_0000_0020;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, bits, false)
                .is_ok()
        });
        assert!(!bridge.handle_shard(bits).lock().raw_py.contains_key(&bits));
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        let address = bridge.address_shard(addr).lock();
        assert!(!address.from_py.contains_key(&addr));
        assert!(!address.direct_molt_py.contains_key(&addr));
    }

    #[test]
    fn static_alias_release_without_reverse_detaches_before_republication() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let old_bits = 0xA110_0000_0000_0090;
        let new_bits = 0xA110_0000_0000_00A0;
        assert!(
            unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, old_bits, false) }.is_ok()
        );
        assert_eq!(
            bridge.release_pyobj(ptr),
            PyObjRelease::DirectViewUnregistered
        );
        {
            let address = bridge.address_shard(addr).lock();
            assert!(!address.from_py.contains_key(&addr));
            assert!(!address.direct_molt_py.contains_key(&addr));
        }
        assert!(unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, new_bits, true) }.is_ok());
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, old_bits) });
        assert_eq!(
            bridge.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle),
            Some(new_bits)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, new_bits) });
    }

    #[test]
    fn static_alias_release_preserves_managed_canonical_projection() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(20_002).bits();
        let managed = unsafe { bridge.owned_handle_to_pyobj(bits) };
        // Direct semantic ingress is not a transfer of managed ownership.
        bridge
            .address_shard(managed.addr())
            .lock()
            .direct_molt_py
            .insert(managed.addr(), bits);
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(managed, bits) });
        let replacement = MoltObject::from_int(20_003).bits();
        for canonical in [false, true] {
            assert_eq!(
                unsafe {
                    bridge.bind_static_pyobj_to_runtime_handle(managed, replacement, canonical)
                },
                Err(StaticBindingError::ManagedPrevious {
                    bits,
                    address: managed.addr()
                })
            );
        }
        let mut alias = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut alias;
        assert_eq!(
            unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, true) },
            Err(StaticBindingError::CanonicalTargetManaged {
                address: managed.addr()
            })
        );
        assert!(unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, false) }.is_ok());
        assert_eq!(
            bridge.release_pyobj(ptr),
            PyObjRelease::DirectViewUnregistered
        );
        assert_eq!(bridge.managed_handle_for_pyobj(managed), Some(bits));
        assert_eq!(unsafe { bridge.handle_to_borrowed_pyobj(bits) }, managed);
        assert_eq!(
            bridge
                .address_shard(managed.addr())
                .lock()
                .direct_molt_py
                .get(&managed.addr())
                .copied(),
            Some(bits)
        );
        assert!(bridge.molt_handle_for_pyobj(ptr).is_none());
        assert_eq!(
            bridge.release_pyobj(managed),
            PyObjRelease::ManagedViewRetired
        );
        assert!(
            !bridge
                .address_shard(managed.addr())
                .lock()
                .direct_molt_py
                .contains_key(&managed.addr())
        );
    }

    #[test]
    fn static_binding_unbind_preserves_newer_forward_identity() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let old_bits = 0xA110_0000_0000_0030;
        let new_bits = 0xA110_0000_0000_0040;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, old_bits, true)
                .is_ok()
        });
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, new_bits, true)
                .is_ok()
        });
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, old_bits) });
        let address = bridge.address_shard(addr).lock();
        assert_eq!(address.from_py.get(&addr), Some(&new_bits));
        assert_eq!(address.direct_molt_py.get(&addr), Some(&new_bits));
        drop(address);
        assert_eq!(
            bridge
                .handle_shard(new_bits)
                .lock()
                .raw_py
                .get(&new_bits)
                .map(|binding| &binding.address),
            Some(&addr)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, new_bits) });
    }

    #[test]
    fn static_binding_unbind_rejects_inconsistent_forward_pair_without_mutation() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let bits = 0xA110_0000_0000_0048;
        let conflicting_bits = 0xA110_0000_0000_0049;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, bits, true)
                .is_ok()
        });
        bridge
            .address_shard(addr)
            .lock()
            .direct_molt_py
            .insert(addr, conflicting_bits);
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        let address = bridge.address_shard(addr).lock();
        assert_eq!(address.from_py.get(&addr), Some(&bits));
        assert_eq!(address.direct_molt_py.get(&addr), Some(&conflicting_bits));
        drop(address);
        assert_eq!(
            unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, true) },
            Err(StaticBindingError::AddressIdentityConflict {
                forward: Some(bits),
                direct: Some(conflicting_bits),
                foreign: None,
                foreign_inflight: false,
                numeric_carrier: None,
            })
        );
        let mut address = bridge.address_shard(addr).lock();
        assert_eq!(address.from_py.get(&addr), Some(&bits));
        assert_eq!(address.direct_molt_py.get(&addr), Some(&conflicting_bits));
        address.direct_molt_py.insert(addr, bits);
        drop(address);
        assert_eq!(
            bridge
                .handle_shard(bits)
                .lock()
                .raw_py
                .get(&bits)
                .map(|binding| &binding.address),
            Some(&addr)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
    }

    #[test]
    fn static_binding_unbind_preserves_newer_reverse_identity() {
        let bridge = ObjectBridge::new();
        let mut canonical = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let mut alias = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let canonical_ptr = &raw mut canonical;
        let alias_ptr = &raw mut alias;
        let canonical_addr = canonical_ptr.addr();
        let alias_addr = alias_ptr.addr();
        let bits = 0xA110_0000_0000_0050;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(canonical_ptr, bits, true)
                .is_ok()
        });
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(alias_ptr, bits, false)
                .is_ok()
        });
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(alias_ptr, bits) });
        assert_eq!(
            bridge
                .handle_shard(bits)
                .lock()
                .raw_py
                .get(&bits)
                .map(|binding| &binding.address),
            Some(&canonical_addr)
        );
        assert!(
            !bridge
                .address_shard(alias_addr)
                .lock()
                .from_py
                .contains_key(&alias_addr)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(canonical_ptr, bits) });
    }

    #[test]
    fn static_binding_unbind_isolated_from_other_identity_classes() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let mut first = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let mut second = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let first_ptr = &raw mut first;
        let second_ptr = &raw mut second;
        let first_bits = 0xA110_0000_0000_0060;
        let second_bits = 0xA110_0000_0000_0070;
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(first_ptr, first_bits, true)
                .is_ok()
        });
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(second_ptr, second_bits, true)
                .is_ok()
        });
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(first_ptr, first_bits) });
        assert_eq!(
            bridge
                .address_shard(second_ptr.addr())
                .lock()
                .direct_molt_py
                .get(&second_ptr.addr()),
            Some(&second_bits)
        );

        let managed_bits = MoltObject::from_int(20_001).bits();
        let managed_ptr = unsafe { bridge.owned_handle_to_pyobj(managed_bits) };
        assert!(!unsafe {
            bridge.unbind_static_pyobj_from_runtime_handle(managed_ptr, managed_bits)
        });
        assert_eq!(
            bridge
                .pyobj_to_handle(managed_ptr)
                .map(BridgeIdentity::as_handle),
            Some(managed_bits)
        );

        let mut foreign = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let foreign_ptr = &raw mut foreign;
        let foreign_bits = 0xA110_0000_0000_0080;
        bridge.insert_foreign_for_test(foreign_ptr, foreign_bits);
        assert!(!unsafe {
            bridge.unbind_static_pyobj_from_runtime_handle(foreign_ptr, foreign_bits)
        });
        assert_eq!(
            bridge
                .address_shard(foreign_ptr.addr())
                .lock()
                .foreign
                .get(&foreign_ptr.addr()),
            Some(&foreign_bits)
        );

        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(second_ptr, second_bits) });
        assert_eq!(
            bridge.release_pyobj(managed_ptr),
            PyObjRelease::ManagedViewRetired
        );
        unsafe { bridge.release_foreign(foreign_ptr.addr()) };
    }

    #[test]
    fn managed_type_retirement_and_rollback_revoke_subclass_identity() {
        use crate::api::typeobj::subclass_registry_tests::{
            assert_type_address_retired, register_subclass_for_test,
        };

        for allocated_heap in [false, true] {
            for exit in ["drop", "callbacks", "release", "deferred", "rollback"] {
                let mut base: PyTypeObject = unsafe { std::mem::zeroed() };
                let mut child: PyTypeObject = unsafe { std::mem::zeroed() };
                let base_pointer = &raw mut base;
                let child_pointer = &raw mut child;
                let mut prefix: PyTypeObject = unsafe { std::mem::zeroed() };
                prefix.ob_base.ob_base.ob_refcnt = 1;
                if allocated_heap {
                    prefix.tp_flags = crate::abi_types::Py_TPFLAGS_HEAPTYPE;
                }
                let mut view = ManagedView::Type {
                    object: ManagedTypeAllocation::new(prefix),
                    _name: std::ffi::CString::new("retiring.Type").unwrap(),
                };
                let pointer = view.py_obj().cast::<PyTypeObject>();
                let address = pointer.addr();
                unsafe {
                    register_subclass_for_test(base_pointer, pointer);
                    register_subclass_for_test(pointer, child_pointer);
                    // Logical flags cannot change the physical retirement owner.
                    (*pointer).tp_flags ^= crate::abi_types::Py_TPFLAGS_HEAPTYPE;
                }
                match exit {
                    "drop" => drop(view),
                    "callbacks" => {
                        let mut callbacks = 0;
                        view.release_owned_items_with(|_, _| {
                            assert_type_address_retired(address);
                            callbacks += 1;
                            if callbacks == 5 {
                                // A last release can observe the live allocation
                                // again; final Box retirement must still revoke it.
                                unsafe {
                                    register_subclass_for_test(base_pointer, pointer);
                                    register_subclass_for_test(pointer, child_pointer);
                                }
                            }
                        });
                        assert_eq!(callbacks, 5);
                        drop(view);
                    }
                    _ => {
                        let bridge = ObjectBridge::new();
                        let bits = MoltObject::from_int(31_006).bits();
                        let guard = (exit == "rollback")
                            .then(|| PublicationBuildGuard::enter(&bridge, bits).unwrap());
                        let entry = Box::new(BridgeEntry {
                            view,
                            bits,
                            unicode: None,
                            publication: if guard.is_some() {
                                PublicationState::Building {
                                    owner: std::thread::current().id(),
                                }
                            } else {
                                PublicationState::Ready
                            },
                            lifecycle: BridgeLifecycle::RuntimeOwned,
                        });
                        {
                            let (mut addresses, mut handles) =
                                bridge.lock_address_then_handle(address, bits);
                            addresses.from_py.insert(address, bits);
                            handles.to_py.insert(bits, entry);
                        }
                        if let Some(guard) = guard {
                            guard.inserted(pointer.cast());
                            drop(guard);
                        } else if exit == "deferred" {
                            assert!(bridge.retire_runtime_type_views(&[bits]));
                        } else {
                            assert_eq!(
                                bridge.release_pyobj(pointer.cast()),
                                PyObjRelease::ManagedViewRetired
                            );
                        }
                        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
                    }
                }
                assert_type_address_retired(address);
                crate::api::typeobj::unregister_type_address(base_pointer.addr());
                crate::api::typeobj::unregister_type_address(child_pointer.addr());
            }
        }
    }

    #[test]
    fn managed_type_extent_and_name_ownership_ignore_mutated_flags() {
        for allocated_heap in [false, true] {
            let mut prefix: PyTypeObject = unsafe { std::mem::zeroed() };
            prefix.tp_flags = if allocated_heap {
                crate::abi_types::Py_TPFLAGS_HEAPTYPE
            } else {
                0
            };
            let allocation = ManagedTypeAllocation::new(prefix);
            let pointer = allocation.get();
            let mut first = PyObject {
                ob_refcnt: 3,
                ob_type: std::ptr::null_mut(),
            };
            let mut second = PyObject {
                ob_refcnt: 4,
                ob_type: std::ptr::null_mut(),
            };
            if let Some(heap) = allocation.heap() {
                unsafe {
                    (*heap).ht_name = &raw mut first;
                    (*heap).ht_qualname = &raw mut second;
                }
            }
            unsafe {
                (*pointer).tp_flags ^= crate::abi_types::Py_TPFLAGS_HEAPTYPE;
            }
            assert_eq!(allocation.heap().is_some(), allocated_heap);
            let mut view = ManagedView::Type {
                object: allocation,
                _name: std::ffi::CString::new("storage.Type").unwrap(),
            };
            let mut released = Vec::new();
            view.release_owned_items_with(|edge, mirrored| {
                if !edge.is_null() {
                    released.push((edge, mirrored));
                }
            });
            if allocated_heap {
                assert_eq!(
                    released,
                    vec![(&raw mut first, true), (&raw mut second, true)]
                );
            } else {
                assert!(released.is_empty());
            }
            released.clear();
            view.release_owned_items_with(|edge, mirrored| {
                if !edge.is_null() {
                    released.push((edge, mirrored));
                }
            });
            assert!(released.is_empty(), "physical owners retire exactly once");
        }
    }

    #[test]
    fn type_publication_validates_exact_noncanonical_binding() {
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(31004).bits();
        let rebound = MoltObject::from_int(31005).bits();
        let mut shell: PyTypeObject = unsafe { std::mem::zeroed() };
        let pointer = &raw mut shell;
        unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(pointer.cast(), bits, false)
                .unwrap();
        }
        {
            let (address, handle) = bridge.lock_address_then_handle(pointer.addr(), bits);
            assert!(ObjectBridge::type_projection_matches(
                &address, &handle, bits, pointer, None, false
            ));
        }
        unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(pointer.cast(), rebound, false)
                .unwrap();
        }
        {
            let (address, handle) = bridge.lock_address_then_handle(pointer.addr(), bits);
            assert!(!ObjectBridge::type_projection_matches(
                &address, &handle, bits, pointer, None, false
            ));
        }
        unsafe {
            assert!(bridge.unbind_static_pyobj_from_runtime_handle(pointer.cast(), rebound));
        }
    }

    #[test]
    fn type_projection_query_includes_noncanonical_static_bindings() {
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(31003).bits();
        let mut shell = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let pointer = &raw mut shell;
        assert!(!bridge.type_has_projection(bits));
        unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(pointer, bits, false)
                .unwrap();
        }
        assert!(bridge.type_has_projection(bits));
        unsafe {
            assert!(bridge.unbind_static_pyobj_from_runtime_handle(pointer, bits));
        }
        assert!(!bridge.type_has_projection(bits));
    }

    #[test]
    fn runtime_type_view_retirement_invalidates_only_managed_type_identity() {
        let bridge = ObjectBridge::new();
        let type_bits = MoltObject::from_int(31_001).bits();
        let mut type_object = unsafe { std::mem::zeroed::<PyTypeObject>() };
        type_object.ob_base.ob_base.ob_refcnt = 7;
        let type_view = ManagedView::Type {
            object: ManagedTypeAllocation::new(type_object),
            _name: std::ffi::CString::new("retiring.Type").unwrap(),
        };
        let type_ptr = type_view.py_obj();
        let type_addr = type_ptr.addr();
        let type_entry = Box::new(BridgeEntry {
            view: type_view,
            bits: type_bits,
            unicode: None,
            publication: PublicationState::Ready,
            lifecycle: BridgeLifecycle::RuntimeOwned,
        });
        {
            let (mut address, mut handle) = bridge.lock_address_then_handle(type_addr, type_bits);
            address.from_py.insert(type_addr, type_bits);
            address.direct_molt_py.insert(type_addr, type_bits);
            handle.to_py.insert(type_bits, type_entry);
        }

        let object_bits = MoltObject::from_int(31_002).bits();
        let object_view = ManagedView::Object(Box::new(BridgeHeader {
            py_obj: UnsafeCell::new(PyObject {
                ob_refcnt: 1,
                ob_type: std::ptr::null_mut(),
            }),
        }));
        let object_ptr = object_view.py_obj();
        let object_addr = object_ptr.addr();
        let object_entry = Box::new(BridgeEntry {
            view: object_view,
            bits: object_bits,
            unicode: None,
            publication: PublicationState::Ready,
            lifecycle: BridgeLifecycle::ViewHoldOnly,
        });
        {
            let (mut address, mut handle) =
                bridge.lock_address_then_handle(object_addr, object_bits);
            address.from_py.insert(object_addr, object_bits);
            address.direct_molt_py.insert(object_addr, object_bits);
            handle.to_py.insert(object_bits, object_entry);
        }

        let mut static_object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let static_ptr = &raw mut static_object;
        let static_addr = static_ptr.addr();
        let static_bits = 0xA110_0000_0000_0090;
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(static_ptr, static_bits, true)
                .is_ok()
        });

        assert!(!bridge.retire_runtime_type_views(&[object_bits]));
        assert_eq!(
            bridge
                .pyobj_to_handle(object_ptr)
                .map(BridgeIdentity::as_handle),
            Some(object_bits)
        );

        assert!(bridge.retire_runtime_type_views(&[type_bits]));
        assert!(bridge.pyobj_to_handle(type_ptr).is_none());
        assert!(
            !bridge
                .address_shard(type_addr)
                .lock()
                .direct_molt_py
                .contains_key(&type_addr)
        );
        assert!(
            !bridge
                .handle_shard(type_bits)
                .lock()
                .to_py
                .contains_key(&type_bits)
        );
        assert_eq!(
            bridge
                .address_shard(static_addr)
                .lock()
                .from_py
                .get(&static_addr),
            Some(&static_bits)
        );
        assert_eq!(
            bridge
                .handle_shard(static_bits)
                .lock()
                .raw_py
                .get(&static_bits)
                .map(|binding| &binding.address),
            Some(&static_addr)
        );
        assert_eq!(
            bridge.release_pyobj(object_ptr),
            PyObjRelease::ManagedViewRetired
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(static_ptr, static_bits) });
    }
}

#[cfg(test)]
mod exception_projection_tests {
    use super::*;
    use molt_lang_obj_model::ExceptionFieldStorage;

    fn valid_snapshot(kind: ExceptionLayoutKind) -> crate::hooks::ExceptionSnapshot {
        let mut snapshot = crate::hooks::ExceptionSnapshot {
            layout_kind: kind as u8,
            present_mask: crate::hooks::EXCEPTION_SNAPSHOT_ARGS,
            args: 0x100,
            ..crate::hooks::ExceptionSnapshot::default()
        };
        for (index, policy) in kind.field_policies().iter().enumerate() {
            if matches!(
                policy.storage,
                ExceptionFieldStorage::Object | ExceptionFieldStorage::RuntimeMessage
            ) {
                snapshot.typed_present_mask |= 1 << index;
                snapshot.typed_handles[index] = 0x200 + index as u64;
            }
        }
        snapshot
    }

    #[test]
    fn every_layout_allocates_exact_shape_and_round_trips_typed_state() {
        let mut fake_type: PyTypeObject = unsafe { std::mem::zeroed() };
        for (raw, kind) in ExceptionLayoutKind::ALL.iter().copied().enumerate() {
            fake_type.tp_basicsize = crate::abi_types::exception_layout_basicsize(kind);
            let mut allocation = ExceptionAllocation::new(kind, 7, &raw mut fake_type);
            assert_eq!(allocation.layout_kind(), kind);
            let base = unsafe { allocation.base() };
            assert_eq!(base.ob_base.ob_refcnt, 7);
            assert_eq!(base.ob_base.ob_type, &raw mut fake_type);

            let mut next = ExceptionViewState::empty(kind);
            next.base[1] = 0x1000usize as *mut PyObject;
            for (index, policy) in kind.field_policies().iter().enumerate() {
                if policy.storage != ExceptionFieldStorage::PySsize {
                    next.typed[index] = (0x2000usize + index * 16) as *mut PyObject;
                }
            }
            if kind == ExceptionLayoutKind::Unicode {
                next.unicode_start = 3;
                next.unicode_end = 9;
            }
            if kind == ExceptionLayoutKind::OSError {
                next.os_error_written = 17;
            }
            let old = unsafe { allocation.replace_state(next) }.unwrap();
            assert_eq!(old, ExceptionViewState::empty(kind));
            assert_eq!(unsafe { allocation.state() }, next);
            let wrong = ExceptionLayoutKind::ALL[(raw + 1) % ExceptionLayoutKind::ALL.len()];
            assert!(
                unsafe { allocation.replace_state(ExceptionViewState::empty(wrong)) }.is_none()
            );
        }
    }

    #[test]
    fn snapshot_validation_is_layout_exact_and_scalar_aware() {
        for (raw, kind) in ExceptionLayoutKind::ALL.iter().copied().enumerate() {
            let snapshot = valid_snapshot(kind);
            assert_eq!(snapshot.validated_layout(Some(kind)), Some(kind));

            let mut mismatch = snapshot;
            mismatch.layout_kind =
                ExceptionLayoutKind::ALL[(raw + 1) % ExceptionLayoutKind::ALL.len()] as u8;
            assert_eq!(mismatch.validated_layout(Some(kind)), None);

            let mut scalar_as_handle = snapshot;
            if let Some(index) = kind
                .field_policies()
                .iter()
                .position(|policy| policy.storage == ExceptionFieldStorage::PySsize)
            {
                scalar_as_handle.typed_present_mask |= 1 << index;
                scalar_as_handle.typed_handles[index] = 0xBAD;
                assert_eq!(scalar_as_handle.validated_layout(Some(kind)), None);
            }
        }
    }

    #[test]
    fn snapshot_presence_is_independent_of_zero_payload_for_every_layout() {
        use crate::hooks::{
            EXCEPTION_BASE_FIELD_MASKS, EXCEPTION_SNAPSHOT_ARGS, ExceptionSnapshot,
        };

        for kind in ExceptionLayoutKind::ALL {
            let mut snapshot = ExceptionSnapshot {
                layout_kind: kind as u8,
                present_mask: EXCEPTION_BASE_FIELD_MASKS.into_iter().fold(0, |a, b| a | b),
                ..ExceptionSnapshot::default()
            };
            let mut expected_edges = EXCEPTION_BASE_FIELD_MASKS.len();
            for (index, policy) in kind.field_policies().iter().enumerate() {
                if policy.storage != ExceptionFieldStorage::PySsize {
                    snapshot.typed_present_mask |= 1 << index;
                    expected_edges += 1;
                }
            }
            assert_eq!(snapshot.validated_layout(Some(kind)), Some(kind));
            assert_eq!(
                snapshot.present_handles().collect::<Vec<_>>(),
                vec![0; expected_edges]
            );
            for mask in EXCEPTION_BASE_FIELD_MASKS {
                let mut absent = snapshot;
                absent.present_mask &= !mask;
                assert_eq!(absent.present_handles().count(), expected_edges - 1);
                assert_eq!(
                    absent.validated_layout(Some(kind)).is_some(),
                    mask != EXCEPTION_SNAPSHOT_ARGS
                );
            }
            for (index, policy) in kind.field_policies().iter().enumerate() {
                let mut absent = snapshot;
                absent.typed_present_mask &= !(1 << index);
                assert_eq!(absent.validated_layout(Some(kind)), Some(kind));
                absent.typed_handles[index] = 1;
                assert_eq!(
                    absent.validated_layout(Some(kind)),
                    None,
                    "unowned payload {policy:?}"
                );
            }
            let mut malformed = snapshot;
            malformed.present_mask &= !crate::hooks::EXCEPTION_SNAPSHOT_NOTES;
            malformed.notes = 1;
            assert_eq!(malformed.validated_layout(Some(kind)), None);
            let mut malformed = snapshot;
            malformed.present_mask |= 1 << 31;
            assert_eq!(malformed.validated_layout(Some(kind)), None);
            let mut malformed = snapshot;
            malformed.typed_present_mask |= 1 << 31;
            assert_eq!(malformed.validated_layout(Some(kind)), None);
        }
    }
}

#[cfg(test)]
mod static_binding_transaction_tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    type Pause = (mpsc::SyncSender<()>, mpsc::Receiver<()>);
    thread_local! {
        static PAUSE: RefCell<Option<Pause>> = const { RefCell::new(None) };
        static FOREIGN_PAUSE: RefCell<Option<Pause>> = const { RefCell::new(None) };
    }

    pub(super) fn pause_after_forward_publication() {
        PAUSE.with(|slot| {
            if let Some((arrived, resume)) = slot.borrow_mut().take() {
                arrived.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
    }

    pub(super) fn pause_before_foreign_reservation() {
        FOREIGN_PAUSE.with(|slot| {
            if let Some((arrived, resume)) = slot.borrow_mut().take() {
                arrived.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
    }

    #[test]
    fn foreign_reservation_revalidates_canonical_publication_after_lookup_miss() {
        let bridge = Arc::new(ObjectBridge::new());
        let address = 0x89000usize;
        let bits = MoltObject::from_int(23_004).bits();
        let (arrived_tx, arrived_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let crossing = Arc::clone(&bridge);
        let worker = thread::spawn(move || {
            FOREIGN_PAUSE.with(|slot| *slot.borrow_mut() = Some((arrived_tx, resume_rx)));
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(address);
            unsafe { crossing.acquire_runtime_value(ptr, RuntimeValueAccess::Observe) }
                .map(|value| (value.bits(), value.owned))
        });
        arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(address);
        assert!(unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, true) }.is_ok());
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Some((bits, false)));
        {
            let shard = bridge.address_shard(address).lock();
            assert_eq!(shard.direct_molt_py.get(&address), Some(&bits));
            assert!(!shard.foreign.contains_key(&address));
            assert!(!shard.foreign_inflight.contains(&address));
        }
        assert_eq!(
            bridge.handle_shard(bits).lock().raw_py.get(&bits).copied(),
            Some(RawBinding::borrowed(address))
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
    }

    fn rebind_with_competitor(old_bits: AbiHandle, bits: AbiHandle, managed_publication: bool) {
        let bridge = Arc::new(ObjectBridge::new());
        let addr = 0x88000usize;
        let candidate = managed_publication.then(|| {
            Box::new(BridgeEntry {
                view: ManagedView::Object(Box::new(BridgeHeader {
                    py_obj: UnsafeCell::new(PyObject {
                        ob_refcnt: 1,
                        ob_type: std::ptr::null_mut(),
                    }),
                })),
                bits,
                unicode: None,
                publication: PublicationState::Ready,
                lifecycle: BridgeLifecycle::ViewHoldOnly,
            })
        });
        let other_addr = candidate.as_ref().map(|entry| entry.view.py_obj().addr());
        let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, old_bits, true)
                .is_ok()
        });
        let (arrived_tx, arrived_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let publisher = Arc::clone(&bridge);
        let writer = thread::spawn(move || {
            PAUSE.with(|slot| *slot.borrow_mut() = Some((arrived_tx, resume_rx)));
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            unsafe {
                publisher
                    .bind_static_pyobj_to_runtime_handle(ptr, bits, true)
                    .is_ok()
            }
        });
        arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // At the old split-publication boundary, all three identities must
        // still be locked. This asserts exclusion without scheduler timing.
        assert!(bridge.address_shard(addr).try_lock().is_none());
        assert!(bridge.handle_shard(old_bits).try_lock().is_none());
        assert!(bridge.handle_shard(bits).try_lock().is_none());
        let competing = Arc::clone(&bridge);
        let start = Arc::new(Barrier::new(2));
        let competing_start = Arc::clone(&start);
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let contender = thread::spawn(move || {
            competing_start.wait();
            let result = if let Some(candidate) = candidate {
                // Exercise the sole managed-view insertion transaction. The
                // competing canonical binding must reject this physical entry
                // before runtime hooks or any publication can run.
                match competing.insert_managed_entry(bits, candidate) {
                    Err((rejected, ManagedEntryRejection::Occupied)) => {
                        assert_eq!(rejected.bits, bits);
                        assert_eq!(Some(rejected.view.py_obj().addr()), other_addr);
                        // The transaction returned custody after its locks
                        // dropped; this unpublished header owns no runtime hold.
                        drop(rejected);
                        false
                    }
                    Err((_, rejection)) => panic!("unexpected rejection: {rejection:?}"),
                    Ok(()) => panic!("managed view replaced a canonical static binding"),
                }
            } else {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
                unsafe { competing.unbind_static_pyobj_from_runtime_handle(ptr, bits) }
            };
            done_tx.send(result).unwrap();
        });
        start.wait();
        resume_tx.send(()).unwrap();
        assert!(writer.join().unwrap());
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            !managed_publication
        );
        contender.join().unwrap();
        assert!(
            !bridge
                .handle_shard(old_bits)
                .lock()
                .raw_py
                .contains_key(&old_bits)
        );
        if managed_publication {
            assert_eq!(
                bridge.handle_shard(bits).lock().raw_py.get(&bits).copied(),
                Some(RawBinding::borrowed(addr))
            );
            assert_eq!(
                bridge.address_shard(addr).lock().from_py.get(&addr),
                Some(&bits)
            );
            assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
            assert_eq!(
                bridge.address_shard(addr).lock().direct_molt_py.get(&addr),
                Some(&bits)
            );
            let other_addr = other_addr.expect("managed competitor has a physical header");
            {
                let rejected_address = bridge.address_shard(other_addr).lock();
                assert!(!rejected_address.from_py.contains_key(&other_addr));
                assert!(!rejected_address.direct_molt_py.contains_key(&other_addr));
            }
            assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        } else {
            let address = bridge.address_shard(addr).lock();
            assert!(!address.from_py.contains_key(&addr));
            assert!(!address.direct_molt_py.contains_key(&addr));
            assert!(!bridge.handle_shard(bits).lock().raw_py.contains_key(&bits));
        }
    }

    fn rebind_cases() -> Vec<(AbiHandle, AbiHandle)> {
        let count = ObjectBridge::new().shard_count() as u64;
        // Distinct identities sharing one shard exercise deduplication.
        let mut cases = vec![(0x1000, 0x1000 + count * 0x10)];
        if count > 1 {
            // Both orders exercise deterministic ordering of distinct shards.
            cases.extend([(0x1000, 0x1010), (0x1010, 0x1000)]);
        }
        cases
    }

    #[test]
    fn static_rebinding_serializes_managed_publication() {
        for (old_bits, bits) in rebind_cases() {
            rebind_with_competitor(old_bits, bits, true);
        }
    }

    #[test]
    fn static_rebinding_serializes_unbinding_without_orphan_reverse_entries() {
        for (old_bits, bits) in rebind_cases() {
            rebind_with_competitor(old_bits, bits, false);
        }
    }
}

#[cfg(test)]
mod bridge_concurrency_tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn stripe_count_and_hashes_follow_the_design() {
        let bridge = ObjectBridge::new();
        #[cfg(target_arch = "wasm32")]
        assert_eq!(bridge.shard_count(), 1);
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(
            bridge.shard_count(),
            std::thread::available_parallelism()
                .map_or(1, usize::from)
                .saturating_mul(2)
                .next_power_of_two()
        );
        let mask = bridge.shard_count() - 1;
        assert_eq!(
            bridge.address_shard_index(0x1234_5670),
            (0x0123_4567_usize) & mask
        );
        assert_eq!(
            bridge.handle_shard_index(0x7ff8_1234_5678_9ab0),
            (0x07ff_8123_4567_89ab_usize) & mask
        );
    }

    #[test]
    fn crossed_stripes_obey_address_then_handle_rank_without_deadlock() {
        let bridge = Arc::new(ObjectBridge::new());
        if bridge.shard_count() == 1 {
            return;
        }
        let barrier = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = mpsc::channel();
        for (addr, bits) in [(0x10usize, 0x20u64), (0x20usize, 0x10u64)] {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let done_tx = done_tx.clone();
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..100_000 {
                    let (_address, _handle) = bridge.lock_address_then_handle(addr, bits);
                }
                done_tx.send(()).expect("rank stress receiver dropped");
            });
        }
        for _ in 0..2 {
            done_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("crossed stripe acquisition deadlocked");
        }
    }

    #[test]
    fn disjoint_crossing_and_release_preserve_bidirectional_identity() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let thread_count = thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(2, 16);
        let barrier = Arc::new(Barrier::new(thread_count));
        let (done_tx, done_rx) = mpsc::channel();
        for thread_index in 0..thread_count {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let done_tx = done_tx.clone();
            thread::spawn(move || {
                barrier.wait();
                for iteration in 0..2_000usize {
                    let ordinal = thread_index * 2_000 + iteration + 1;
                    let address = 0x1_0000usize + ordinal * 16;
                    let heap_bits = MoltObject::from_ptr(address as *mut u8).bits();
                    let value = 1_000 + (thread_index * 2_000 + iteration) as i64;
                    let numeric_bits = MoltObject::from_int(value).bits();
                    // Exercise both heap-handle and non-small numeric managed
                    // views through the one publication/release authority.
                    for bits in [heap_bits, numeric_bits] {
                        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
                        assert_eq!(
                            bridge.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle),
                            Some(bits)
                        );
                        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
                    }
                }
                done_tx.send(()).expect("crossing stress receiver dropped");
            });
        }
        for _ in 0..thread_count {
            done_rx
                .recv_timeout(Duration::from_secs(20))
                .expect("concurrent bridge crossing deadlocked");
        }
    }

    #[test]
    fn small_int_crossings_are_stateless_immortal_and_deterministic() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        for value in -5..=256 {
            let bits = MoltObject::from_int(value).bits();
            let first = unsafe { bridge.owned_handle_to_pyobj(bits) };
            let second = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
            assert_eq!(first, second);
            assert!(unsafe { crate::abi_types::is_immortal_refcnt((*first).ob_refcnt) });
            assert_eq!(
                bridge.pyobj_to_handle(first).map(BridgeIdentity::as_handle),
                Some(bits)
            );
            assert_eq!(bridge.release_pyobj(first), PyObjRelease::StaticImmortal);
        }
        assert!(
            bridge
                .address_shards
                .iter()
                .all(|shard| shard.lock().from_py.is_empty())
        );
        assert!(
            bridge
                .handle_shards
                .iter()
                .all(|shard| shard.lock().to_py.is_empty())
        );
    }
}

#[cfg(test)]
mod bridge_publication_race_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn same_handle_crossing_publishes_one_ready_pointer() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(20_000).bits();
        let workers = 8;
        let barrier = Arc::new(Barrier::new(workers));
        let (tx, rx) = mpsc::channel();
        for _ in 0..workers {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            thread::spawn(move || {
                barrier.wait();
                let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
                tx.send(ptr.addr()).expect("same-handle receiver dropped");
            });
        }
        let pointers = (0..workers)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<Vec<_>>();
        assert!(pointers.iter().all(|ptr| *ptr == pointers[0]));
        let handle = bridge.handle_shard(bits).lock();
        assert!(matches!(
            handle.to_py.get(&bits).map(|entry| &entry.publication),
            Some(PublicationState::Ready)
        ));
        drop(handle);
        let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(pointers[0]);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    fn install_building_view(
        bridge: &ObjectBridge,
        bits: AbiHandle,
    ) -> (*mut PyObject, PublicationBuildGuard<'_>) {
        let guard = PublicationBuildGuard::enter(bridge, bits).unwrap();
        let (entry, ptr) = unsafe { bridge.build_pyobj_entry(bits, 2, false) }.unwrap();
        bridge
            .insert_managed_entry(bits, entry)
            .map_err(|(_, error)| error)
            .unwrap();
        guard.inserted(ptr);
        (ptr, guard)
    }

    #[test]
    fn building_publication_blocks_other_threads_until_ready() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(30_000).bits();
        let (ptr, guard) = install_building_view(&bridge, bits);
        // The owner may re-enter to materialize recursive projections.
        assert_eq!(
            bridge.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle),
            Some(bits)
        );
        let (tx, rx) = mpsc::channel();
        let waiter = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(waiter.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle))
                .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(guard.finish());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(bits));
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn publication_rollback_wakes_waiters_and_removes_both_directions() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(40_000).bits();
        let (ptr, guard) = install_building_view(&bridge, bits);
        let (tx, rx) = mpsc::channel();
        let waiter = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(waiter.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle))
                .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(guard);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        assert!(
            !bridge
                .address_shard(addr)
                .lock()
                .from_py
                .contains_key(&addr)
        );
        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
    }

    #[test]
    fn release_waits_for_building_projection_before_retirement() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(45_000).bits();
        let (ptr, guard) = install_building_view(&bridge, bits);
        // A direct-ingress hint must not bypass managed publication custody.
        bridge
            .address_shard(ptr.addr())
            .lock()
            .direct_molt_py
            .insert(ptr.addr(), bits);
        let (tx, rx) = mpsc::channel();
        let releaser = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(releaser.release_pyobj(ptr)).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(guard.finish());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PyObjRelease::ManagedViewRetired
        );
        assert!(
            !bridge
                .address_shard(addr)
                .lock()
                .from_py
                .contains_key(&addr)
        );
        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
    }

    #[test]
    fn managed_incref_decref_race_is_linearized_by_bridge_lock() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(50_000).bits();
        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
        let addr = ptr.addr();
        let workers = 8;
        let barrier = Arc::new(Barrier::new(workers));
        let (tx, rx) = mpsc::channel();
        for _ in 0..workers {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            thread::spawn(move || {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
                barrier.wait();
                for _ in 0..5_000 {
                    assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
                    assert_eq!(
                        unsafe { bridge.managed_decref_pyobj(ptr) },
                        ManagedDecref::Alive
                    );
                }
                tx.send(()).unwrap();
            });
        }
        for _ in 0..workers {
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn managed_immortal_view_refcount_operations_are_noops() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_000).bits();
        assert!(!bridge.is_immortal_c_view(bits));
        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
        assert!(!bridge.is_immortal_c_view(bits));
        assert!(unsafe { bridge.projection_incref(ptr) });
        assert_eq!(bridge.mirrored_c_refcount(ptr.addr()), 1);
        unsafe { bridge.set_pyobj_refcnt(ptr, crate::abi_types::IMMORTAL_REFCNT) };
        assert!(bridge.is_immortal_c_view(bits));
        assert_eq!(bridge.mirrored_c_refcount(ptr.addr()), 0);
        unsafe { bridge.set_pyobj_refcnt(ptr, 1) };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        assert_eq!(
            unsafe { bridge.managed_decref_pyobj(ptr) },
            ManagedDecref::Immortal
        );
        assert_eq!(
            unsafe { (*ptr).ob_refcnt },
            crate::abi_types::IMMORTAL_REFCNT
        );
        assert!(bridge.has_direct_c_refs(bits));
        assert_eq!(bridge.gc_ref_adjustment(bits), 0);
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, true, 0, || Some(1)),
            Some(1)
        );
        let release = bridge
            .transition_runtime_owner_release(bits, 0, || 2, || unreachable!())
            .expect("release a runtime owner while the immortal C root remains");
        assert!(!release.should_finalize());
        assert_eq!(
            unsafe { (*ptr).ob_refcnt },
            crate::abi_types::IMMORTAL_REFCNT
        );
        unsafe { bridge.projection_decref(ptr) };
        assert_eq!(bridge.mirrored_c_refcount(ptr.addr()), 0);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
        assert!(!bridge.is_immortal_c_view(bits));
    }

    #[test]
    fn immortal_runtime_view_rebases_once_for_terminal_shutdown() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_001).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        unsafe { (*ptr).ob_refcnt = crate::abi_types::IMMORTAL_REFCNT };

        bridge.prepare_runtime_immortal_for_shutdown(bits);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
        let release = bridge
            .transition_runtime_owner_release(bits, 0, || 1, || {})
            .expect("immortal shutdown owner release");
        assert!(release.should_finalize());
        bridge.begin_finalization(bits);
        bridge.finish_finalization(bits, false);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 0);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn runtime_owner_transition_publishes_terminal_gate_before_unlock() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_002).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        let runtime_refs = AtomicU32::new(2);

        let release = bridge
            .transition_runtime_owner_release(
                bits,
                0,
                || runtime_refs.fetch_sub(1, Ordering::AcqRel),
                || panic!("stable view hold must not be restored for 2 -> 1"),
            )
            .expect("runtime owner release");
        assert_eq!(release.previous(), 2);
        assert!(release.should_finalize());
        assert_eq!(runtime_refs.load(Ordering::Acquire), 1);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);

        let retain_called = std::cell::Cell::new(false);
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, false, 0, || {
                retain_called.set(true);
                Some(runtime_refs.fetch_add(1, Ordering::AcqRel))
            }),
            None,
        );
        assert!(!retain_called.get());
        bridge.begin_finalization(bits);
        bridge.finish_finalization(bits, false);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 0);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn runtime_owner_add_and_drop_update_view_bias_in_one_transaction() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_003).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        let runtime_refs = AtomicU32::new(2);

        let release = bridge
            .transition_runtime_owner_release(
                bits,
                0,
                || runtime_refs.fetch_sub(1, Ordering::AcqRel),
                || panic!("stable view hold must not be restored for 2 -> 1"),
            )
            .expect("runtime owner release");
        assert!(!release.should_finalize());
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, false, 0, || {
                Some(runtime_refs.fetch_add(1, Ordering::AcqRel))
            }),
            Some(1),
        );
        assert_eq!(runtime_refs.load(Ordering::Acquire), 2);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 2);
    }

    #[test]
    fn runtime_owner_transition_excludes_internal_gc_pin_from_liveness() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_005).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        let runtime_refs = AtomicU32::new(2);
        let release = bridge
            .transition_runtime_owner_release(
                bits,
                0,
                || runtime_refs.fetch_sub(1, Ordering::AcqRel),
                || panic!("stable view hold must not be restored for 2 -> 1"),
            )
            .expect("runtime owner release");
        assert!(!release.should_finalize());
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);

        runtime_refs.fetch_add(1, Ordering::AcqRel); // collector pin
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, false, 1, || {
                Some(runtime_refs.fetch_add(1, Ordering::AcqRel))
            }),
            Some(2),
        );
        assert_eq!(runtime_refs.load(Ordering::Acquire), 3);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 2);
    }

    #[test]
    fn concurrent_runtime_owner_add_release_has_only_live_or_terminal_outcomes() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(60_004).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        let runtime_refs = Arc::new(AtomicU32::new(2));
        let start = Arc::new(Barrier::new(2));

        let add_bridge = Arc::clone(&bridge);
        let add_refs = Arc::clone(&runtime_refs);
        let add_start = Arc::clone(&start);
        let add = thread::spawn(move || {
            add_start.wait();
            add_bridge.transition_runtime_owner_add(bits, false, 0, || {
                Some(add_refs.fetch_add(1, Ordering::AcqRel))
            })
        });
        let drop_bridge = Arc::clone(&bridge);
        let drop_refs = Arc::clone(&runtime_refs);
        let drop_start = Arc::clone(&start);
        let release = thread::spawn(move || {
            drop_start.wait();
            drop_bridge
                .transition_runtime_owner_release(
                    bits,
                    0,
                    || drop_refs.fetch_sub(1, Ordering::AcqRel),
                    || panic!("stable view hold must not be restored for this race"),
                )
                .expect("runtime owner release")
        });

        let added = add.join().expect("add worker");
        let released = release.join().expect("release worker");
        match runtime_refs.load(Ordering::Acquire) {
            1 => {
                assert_eq!(added, None);
                assert!(released.should_finalize());
                assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
            }
            2 => {
                assert_eq!(added, Some(2));
                assert_eq!(released.previous(), 3);
                assert!(!released.should_finalize());
                assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
            }
            refs => panic!("non-linearized runtime owner count {refs}"),
        }
    }
}
