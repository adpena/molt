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

mod identity;
mod lifecycle;
mod publication;
mod slice;

use identity::RawBinding;
pub use identity::{BridgeIdentity, MoltValueHandle, StaticBindingError};
pub(crate) use identity::{
    NumericCarrierKind, NumericCarrierRecord, ResolvedPyObject, RuntimeValue, observe_pyobject,
    resolve_pyobject, resolved_molt_handle,
};
use lifecycle::BridgeLifecycle;
pub use lifecycle::{
    CRefZero, ManagedDecref, PyObjRelease, RetiredOwnedCFields, RetiredRuntimeView,
    RetiredTypeCycleProjection, RuntimeOwnerRelease,
};
use publication::PublicationState;
pub(crate) use publication::RuntimeTypeProjection;
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

mod unicode;
use unicode::UnicodeProjection;

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
struct ManagedStaticTypeAllocation {
    object: UnsafeCell<PyTypeObject>,
    protocols: crate::api::typeobj::TypeProtocolTables,
}

enum ManagedTypeAllocation {
    // These tables belong to the projection owner, not to a fictitious heap
    // tail. heap() remains None even if C code changes the semantic flags.
    Static(Box<ManagedStaticTypeAllocation>),
    Heap(Box<UnsafeCell<crate::abi_types::PyHeapTypeObject>>),
}

impl ManagedTypeAllocation {
    fn new(object: PyTypeObject) -> Self {
        if object.tp_flags & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 {
            let mut heap: crate::abi_types::PyHeapTypeObject = unsafe { std::mem::zeroed() };
            heap.ht_type = object;
            let storage = Box::new(UnsafeCell::new(heap));
            unsafe {
                let heap = storage.get();
                (*heap).ht_type.tp_as_async = (&raw mut (*heap).as_async).cast();
                (*heap).ht_type.tp_as_number = (&raw mut (*heap).as_number).cast();
                (*heap).ht_type.tp_as_sequence = (&raw mut (*heap).as_sequence).cast();
                (*heap).ht_type.tp_as_mapping = (&raw mut (*heap).as_mapping).cast();
                (*heap).ht_type.tp_as_buffer = (&raw mut (*heap).as_buffer).cast();
            }
            Self::Heap(storage)
        } else {
            let storage = Box::new(ManagedStaticTypeAllocation {
                object: UnsafeCell::new(object),
                protocols: crate::api::typeobj::TypeProtocolTables::new(),
            });
            unsafe {
                storage.protocols.attach(storage.object.get());
            }
            Self::Static(storage)
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
            Self::Static(storage) => storage.object.get(),
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

struct BridgeEntry {
    view: ManagedView,
    bits: AbiHandle,
    /// The sole object-owned Unicode construction/export projection. It owns
    /// the codepoint data and optional strict UTF-8 encoding cache together.
    unicode: Option<UnicodeProjection>,
    publication: PublicationState,
    lifecycle: BridgeLifecycle,
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

struct HandleShard {
    to_py: HashMap<AbiHandle, Box<BridgeEntry>>,
    raw_py: HashMap<AbiHandle, RawBinding>,
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
}

mod collection_projection;

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
                    return Err(crate::ErrorIndicatorSet);
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
///
/// # Safety
/// A non-null result transfers one owned C reference. Runtime hooks and the
/// bridge must be initialized; failure is returned with the C error preserved.
pub unsafe fn owned_native_result_to_runtime(
    result: *mut PyObject,
) -> crate::hooks::OwnedHandleResult {
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
        Err(crate::ErrorIndicatorSet) => return crate::hooks::OwnedHandleResult::error(),
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
        Err(crate::ErrorIndicatorSet) => return crate::hooks::OwnedHandleResult::error(),
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

/// Attribute lookup on a foreign wrapper. Normal access uses `tp_getattro`;
/// explicit generic access bypasses that
/// override through the C generic lookup authority. `name_bits` is a Molt string
/// handle. The status is separate from the owned value, preserving float +0.0.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_getattr(
    c_ptr: usize,
    name_bits: u64,
    access: crate::hooks::AttributeAccess,
) -> crate::hooks::OwnedHandleResult {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return crate::hooks::OwnedHandleResult::error();
    }
    let name_obj = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name_bits) };
    if name_obj.is_null() {
        return crate::hooks::OwnedHandleResult::error();
    }
    let result = unsafe {
        match access {
            crate::hooks::AttributeAccess::Normal => {
                crate::api::object::native_get_attr(obj, name_obj)
            }
            crate::hooks::AttributeAccess::Generic => {
                crate::api::object::PyObject_GenericGetAttr(obj, name_obj)
            }
        }
    };
    unsafe { crate::api::errors::release_preserving_error(&[name_obj]) };
    unsafe { owned_native_result_to_runtime(result) }
}

/// Read a foreign sequence through its native sq_item and owned-result boundary.
///
/// # Safety
/// `c_ptr` must identify a live native object retained by its runtime wrapper.
pub unsafe fn molt_foreign_sequence_item(
    c_ptr: usize,
    index: isize,
) -> crate::hooks::OwnedHandleResult {
    let object = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    let result = unsafe { crate::api::abstract_sequence::PySequence_GetItem(object, index) };
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

/// The C admission boundary accepts the actual receiver type. Managed callers
/// enter only when their MRO contains a foreign type; no receiver is projected.
///
/// # Safety
/// `type_bits` denotes a live class under the runtime GIL.
pub unsafe fn native_setter_type_admitted(
    type_bits: u64,
    type_default: bool,
    delete: bool,
) -> bool {
    unsafe {
        let owner = crate::api::refcount::OwnedPyObject::from_owned(
            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(type_bits),
        );
        if owner.as_ptr().is_null() {
            return false;
        }
        let wrapped = if type_default {
            crate::api::typeobj::type_setattro as *const ()
        } else {
            crate::api::object::PyObject_GenericSetAttr as *const ()
        };
        let admitted = crate::api::typeobj::setter_type_admitted(
            owner.as_ptr().cast(),
            wrapped as *mut std::ffi::c_void,
            delete,
        );
        crate::api::errors::check_native_status(
            if admitted { 0 } else { -1 },
            "explicit mutation admission",
        ) == 0
    }
}

/// A foreign object already owns its C identity, including any bound native
/// type. Admission must inspect that live slot, not remap it to runtime defaults.
///
/// # Safety
/// `c_ptr` is a live foreign receiver under the runtime GIL.
pub unsafe fn foreign_object_setter_admitted(c_ptr: usize, delete: bool) -> bool {
    unsafe {
        let receiver = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
        let admitted = crate::api::typeobj::setter_admitted(
            receiver,
            crate::api::object::PyObject_GenericSetAttr as *const () as *mut std::ffi::c_void,
            delete,
        );
        crate::api::errors::check_native_status(
            if admitted { 0 } else { -1 },
            "foreign object mutation admission",
        ) == 0
    }
}

/// Prepared publication of a managed namespace into its existing native
/// descendants. Class projection and subclass membership remain bridge/type
/// authorities; this transaction owns no additional registry or cached slots.
pub struct RuntimeTypeMutation {
    class: u64,
    name: u64,
    prepared: Vec<(
        crate::api::refcount::OwnedPyObject,
        Option<crate::api::typeobj::native_slot_mutation::Mutation>,
    )>,
}

impl RuntimeTypeMutation {
    /// Prepare physical declaration names only for existing C type roots.
    /// The caller pins the runtime class and name across dictionary commit.
    ///
    /// # Safety
    /// `class` and `name` must remain live under the GIL through publication.
    pub unsafe fn prepare(class: u64, name: u64) -> Result<Self, crate::ErrorIndicatorSet> {
        let mut result = Self {
            class,
            name,
            prepared: Vec::new(),
        };
        unsafe {
            result.prepare_existing()?;
        }
        Ok(result)
    }

    /// Return the current binding cohort's prepared indices. Keep displaced
    /// roots pinned too: callback-capable commit can add, unbind, or rebind an
    /// alias, but must not retire a captured owner before publication finishes.
    unsafe fn prepare_existing(&mut self) -> Result<Vec<usize>, crate::ErrorIndicatorSet> {
        unsafe {
            let roots = GLOBAL_BRIDGE.existing_type_projections(self.class)?;
            let mut current = Vec::with_capacity(roots.len());
            let mut name = None;
            for root in roots {
                if let Some(index) = self
                    .prepared
                    .iter()
                    .position(|(prepared, _)| prepared.as_ptr() == root.as_ptr())
                {
                    current.push(index);
                    continue;
                }
                if name.is_none() {
                    let mut length = 0;
                    let bytes =
                        (crate::hooks::hooks_or_stubs().str_data)(self.name, &raw mut length);
                    if bytes.is_null() {
                        crate::api::errors::check_native_status(
                            -1,
                            "managed type mutation name bytes",
                        );
                        return Err(crate::ErrorIndicatorSet);
                    }
                    // Pinned canonical string bytes; no Python callback and no
                    // C string view for a non-slot metadata/namespace name.
                    let slot_name = crate::api::typeobj::native_slot_mutation::affects_slots(
                        std::slice::from_raw_parts(bytes, length),
                    );
                    let projected = if slot_name {
                        let projected = crate::api::refcount::OwnedPyObject::from_owned(
                            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(self.name),
                        );
                        if projected.as_ptr().is_null() {
                            return Err(crate::ErrorIndicatorSet);
                        }
                        Some(projected)
                    } else {
                        None
                    };
                    name = Some(projected);
                }
                let slots = match name.as_ref().unwrap() {
                    Some(name) => Some(crate::api::typeobj::native_slot_mutation::prepare(
                        root.as_ptr().cast(),
                        name.as_ptr(),
                    )?),
                    None => None,
                };
                current.push(self.prepared.len());
                self.prepared.push((root, slots));
            }
            Ok(current)
        }
    }

    /// Reobserve after callback-capable commit, before displaced retirement.
    /// Include newly published roots; do not modify an alias rebound to another
    /// class. Every callback runs outside bridge locks with owners retained.
    ///
    /// # Safety
    /// The original runtime class/name remain live under the GIL.
    pub unsafe fn publish(&mut self) -> i32 {
        unsafe {
            let mut published = std::collections::HashSet::new();
            loop {
                let current = match self.prepare_existing() {
                    Ok(current) => current,
                    Err(crate::ErrorIndicatorSet) => return -1,
                };
                let mut advanced = false;
                for index in current {
                    let (root, slots) = &self.prepared[index];
                    let pointer = root.as_ptr();
                    if GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(pointer)
                        .map(|handle| handle.bits())
                        != Some(self.class)
                        || !published.insert(pointer.addr())
                    {
                        continue;
                    }
                    advanced = true;
                    crate::api::typeobj::PyType_Modified(pointer.cast());
                    if GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(pointer)
                        .map(|handle| handle.bits())
                        == Some(self.class)
                        && let Some(slots) = slots
                        && slots.publish_for_runtime(self.class) < 0
                    {
                        return -1;
                    }
                }
                // Watcher and lookup callbacks may admit further roots. Retain
                // and publish that cohort too, once per pinned allocation.
                if !advanced {
                    break;
                }
            }
            0
        }
    }
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
    access: crate::hooks::AttributeMutation,
) -> std::os::raw::c_int {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return -1;
    }
    let operation = match access {
        crate::hooks::AttributeMutation::Normal => crate::api::object::PyObject_SetAttr,
        crate::hooks::AttributeMutation::Generic => crate::api::object::PyObject_GenericSetAttr,
        crate::hooks::AttributeMutation::TypeDefault => crate::api::typeobj::type_setattro,
    };
    unsafe { mutate_projected_attribute(obj, name_bits, value_bits, operation) }
}

/// Explicit type defaults on a foreign receiver use its existing C identity.
/// Managed defaults stay in the runtime and never enter this boundary.
///
/// # Safety
/// `c_ptr` is the live C receiver owned by a foreign runtime wrapper.
pub unsafe fn molt_foreign_type_setattr(
    c_ptr: usize,
    name_bits: u64,
    value_bits: Option<u64>,
) -> std::os::raw::c_int {
    let receiver = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if unsafe { crate::api::typeobj::PyType_Check(receiver) } == 0 {
        let operation = if value_bits.is_some() {
            "__setattr__"
        } else {
            "__delattr__"
        };
        let name =
            unsafe { crate::api::typeobj::object_type_name_with_precision(receiver, usize::MAX) };
        let message = std::ffi::CString::new(format!(
            "descriptor '{operation}' requires a 'type' object but received a '{name}'"
        ))
        .expect("type names are C strings");
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                message.as_ptr(),
            );
        }
        return -1;
    }
    let operation = crate::api::typeobj::type_setattro;
    if unsafe {
        crate::api::typeobj::setter_admitted(
            receiver,
            operation as *const () as *mut std::ffi::c_void,
            value_bits.is_none(),
        )
    } {
        unsafe { mutate_projected_attribute(receiver, name_bits, value_bits, operation) }
    } else {
        unsafe { crate::api::errors::check_native_status(-1, "explicit type mutation admission") }
    }
}

/// Both receiver ownership lanes use one operand projection/retirement owner.
unsafe fn mutate_projected_attribute(
    obj: *mut PyObject,
    name_bits: u64,
    value_bits: Option<u64>,
    operation: unsafe extern "C" fn(
        *mut PyObject,
        *mut PyObject,
        *mut PyObject,
    ) -> std::os::raw::c_int,
) -> std::os::raw::c_int {
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
    let rc = unsafe { operation(obj, name_obj, value_obj) };
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
}
