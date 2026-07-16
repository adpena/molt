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
    MoltManaged_Type, MoltTypeTag, Py_False, Py_None, Py_True, PyBaseExceptionObject,
    PyBaseObject_Type, PyBool_Type, PyList_Type, PyObject, PyTuple_Type, PyType_Type, PyTypeObject,
};
use molt_lang_obj_model::MoltObject;
use once_cell::sync::OnceCell;
use parking_lot::{Condvar, Mutex, MutexGuard};
use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ptr::NonNull;
use std::sync::Once;

/// A MoltHandle cast to u64, used as bridge map key.
pub type AbiHandle = u64;

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
        None => ResolvedPyObject::Foreign,
    })
}

#[inline]
pub(crate) fn resolved_molt_handle(ptr: *mut PyObject) -> Option<MoltValueHandle> {
    GLOBAL_BRIDGE.observed_handle_for_pyobj(ptr)
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
                    ob_base: PyObject {
                        ob_refcnt: if len == 0 {
                            crate::abi_types::IMMORTAL_REFCNT
                        } else {
                            ob_refcnt
                        },
                        ob_type,
                    },
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

enum ManagedView {
    Object(Box<BridgeHeader>),
    Type {
        object: Box<UnsafeCell<PyTypeObject>>,
        _name: std::ffi::CString,
    },
    Tuple {
        allocation: TupleAllocation,
    },
    List {
        allocation: ListAllocation,
    },
    Exception(Box<UnsafeCell<PyBaseExceptionObject>>),
}

unsafe impl Send for ManagedView {}

impl ManagedView {
    fn py_obj(&self) -> *mut PyObject {
        match self {
            Self::Object(header) => header.py_obj.get(),
            Self::Type { object, .. } => object.get().cast::<PyObject>(),
            Self::Tuple { allocation, .. } => allocation.py_obj(),
            Self::List { allocation, .. } => allocation.py_obj(),
            Self::Exception(object) => object.get().cast::<PyObject>(),
        }
    }

    /// Release references owned solely by a concrete-layout sidecar.  This is
    /// deliberately called only after bridge-map locks have been dropped:
    /// carrier deallocation re-enters the address registry.
    fn release_owned_items(&mut self) {
        match self {
            Self::Tuple { allocation } => {
                for pointer in allocation.items().iter().copied().filter(|p| !p.is_null()) {
                    unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
                }
            }
            Self::List { allocation } => {
                for index in 0..allocation.items.len() {
                    let current = allocation.items[index];
                    let shadow = allocation.shadow[index];
                    if !shadow.is_null() {
                        unsafe { GLOBAL_BRIDGE.projection_decref(shadow) };
                    }
                    if current != shadow && !current.is_null() {
                        unsafe { crate::api::refcount::Py_DECREF(current) };
                    }
                }
            }
            Self::Exception(object) => unsafe {
                let object = &mut *object.get();
                for field in [
                    &mut object.dict,
                    &mut object.args,
                    &mut object.notes,
                    &mut object.traceback,
                    &mut object.context,
                    &mut object.cause,
                ] {
                    let value = std::mem::replace(field, std::ptr::null_mut());
                    if !value.is_null() {
                        crate::api::refcount::Py_DECREF(value);
                    }
                }
            },
            Self::Object(_) | Self::Type { .. } => {}
        }
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
    static PUBLICATION_BUILD_STACK: RefCell<ReentrantHandleStack> = const {
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

fn release_bridge_entry(mut entry: BridgeEntry) {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let _ = unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(entry.bits, 0) };
    entry.view.release_owned_items();
}

/// A tuple projection removed from both bridge identity maps but kept alive
/// until the runtime has released the tuple's semantic item edges.  Deferring
/// the physical C-edge decrements prevents a child canonical view from losing
/// its stable runtime hold before the corresponding tuple-owned edge retires.
pub struct RetiredTupleView {
    entry: Option<Box<BridgeEntry>>,
}

impl Drop for RetiredTupleView {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            release_bridge_entry(*entry);
        }
    }
}

struct BridgeEntry {
    view: ManagedView,
    bits: AbiHandle,
    /// CPython-compatible, object-owned UTF-8 cache. The final byte is always
    /// NUL and the payload length excludes it. Dropping the bridge entry drops
    /// this cache, matching the `str` object's C-visible lifetime.
    utf8: Option<Box<[u8]>>,
    publication: PublicationState,
    lifecycle: BridgeLifecycle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PublicationState {
    Building { owner: std::thread::ThreadId },
    Ready,
    Retiring,
}

struct PublicationBuildGuard {
    bits: AbiHandle,
}

impl PublicationBuildGuard {
    fn enter(bits: AbiHandle) -> Self {
        PUBLICATION_BUILD_STACK.with(|stack| stack.borrow_mut().push(bits));
        Self { bits }
    }
}

impl Drop for PublicationBuildGuard {
    fn drop(&mut self) {
        PUBLICATION_BUILD_STACK.with(|stack| {
            let popped = stack.borrow_mut().pop();
            debug_assert_eq!(popped, Some(self.bits));
        });
    }
}

#[inline]
fn is_publication_owner(bits: AbiHandle, owner: &std::thread::ThreadId) -> bool {
    *owner == std::thread::current().id()
        && PUBLICATION_BUILD_STACK.with(|stack| stack.borrow().contains(bits))
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

impl BridgeLifecycle {
    #[inline]
    fn has_c_bias(self) -> bool {
        matches!(self, Self::RuntimeOwned | Self::FinalizingPin)
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
    /// C references owned by concrete ABI projections (list/exception fields),
    /// distinct from externally-held C roots. Cycle GC traverses these edges;
    /// finalization must therefore not mistake them for resurrection.
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

struct HandleShard {
    to_py: HashMap<AbiHandle, Box<BridgeEntry>>,
    raw_py: HashMap<AbiHandle, usize>,
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
/// acquires the address shard first and the handle shard second.
pub struct ObjectBridge {
    address_shards: Box<[Mutex<AddressShard>]>,
    foreign_ready: Box<[Condvar]>,
    handle_shards: Box<[Mutex<HandleShard>]>,
    publication_ready: Box<[Condvar]>,
    shard_mask: usize,
}

unsafe fn ensure_result_error(message: &std::ffi::CStr) {
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
        && !crate::api::errors::transfer_runtime_pending_to_current()
    {
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

    unsafe fn managed_type_metadata(bits: AbiHandle) -> Option<(std::ffi::CString, u64)> {
        let hooks = crate::hooks::hooks_or_stubs();
        let read_attr = |name: &[u8]| -> Option<u64> {
            let name_bits = unsafe { (hooks.alloc_str)(name.as_ptr(), name.len()) };
            if name_bits == 0 {
                return None;
            }
            let result = unsafe { (hooks.object_get_attr)(bits, name_bits) };
            unsafe { (hooks.dec_ref)(name_bits) };
            match result.decode() {
                crate::hooks::DecodedHandleResult::Ok(value_bits) if value_bits != 0 => {
                    Some(value_bits)
                }
                crate::hooks::DecodedHandleResult::Ok(_)
                | crate::hooks::DecodedHandleResult::Missing
                | crate::hooks::DecodedHandleResult::Error => None,
            }
        };
        let read_string = |value_bits: u64| -> Option<String> {
            let mut len = 0usize;
            let data = unsafe { (hooks.str_data)(value_bits, &raw mut len) };
            if data.is_null() {
                return None;
            }
            let bytes = unsafe { std::slice::from_raw_parts(data, len) };
            std::str::from_utf8(bytes).ok().map(str::to_owned)
        };
        let qualname_bits = read_attr(b"__qualname__").or_else(|| read_attr(b"__name__"))?;
        let qualname = read_string(qualname_bits);
        unsafe { (hooks.dec_ref)(qualname_bits) };
        let qualname = qualname?;
        let module = read_attr(b"__module__").and_then(|module_bits| {
            let module = read_string(module_bits);
            unsafe { (hooks.dec_ref)(module_bits) };
            module
        });
        let qualified = match module.as_deref() {
            Some("builtins") | None => qualname,
            Some(module) => format!("{module}.{qualname}"),
        };
        let name = std::ffi::CString::new(qualified).ok()?;

        let bases_bits = read_attr(b"__bases__")?;
        let base_bits = match unsafe { (hooks.tuple_item)(bases_bits, 0) }.decode() {
            crate::hooks::DecodedHandleResult::Ok(base_bits) => base_bits,
            crate::hooks::DecodedHandleResult::Missing => 0,
            crate::hooks::DecodedHandleResult::Error => {
                unsafe { (hooks.dec_ref)(bases_bits) };
                return None;
            }
        };
        unsafe { (hooks.dec_ref)(bases_bits) };
        Some((name, base_bits))
    }
}

// Fallible physical-view construction prior to publication.
impl ObjectBridge {
    unsafe fn build_pyobj_entry(
        &self,
        bits: AbiHandle,
        ob_refcnt: isize,
        internal_c_ref: bool,
    ) -> Option<(Box<BridgeEntry>, *mut PyObject)> {
        let tag = Self::classify_handle(bits);
        let ob_type = if tag == MoltTypeTag::Exception {
            match unsafe { (crate::hooks::hooks_or_stubs().exception_class_borrowed)(bits) }
                .decode()
            {
                crate::hooks::DecodedHandleResult::Ok(class_bits) => {
                    let class_view = unsafe { self.handle_to_borrowed_pyobj(class_bits) };
                    if class_view.is_null() {
                        return None;
                    }
                    class_view.cast::<PyTypeObject>()
                }
                crate::hooks::DecodedHandleResult::Missing
                | crate::hooks::DecodedHandleResult::Error => return None,
            }
        } else {
            unsafe { tag_to_type(tag) }
        };
        let view = if tag == MoltTypeTag::Type {
            let (name, base_bits) = unsafe { Self::managed_type_metadata(bits) }?;
            let base = if base_bits == 0 {
                &raw mut PyBaseObject_Type
            } else {
                let base_view = unsafe { self.handle_to_borrowed_pyobj(base_bits) };
                if base_view.is_null() {
                    return None;
                }
                base_view.cast::<PyTypeObject>()
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
            object.tp_basicsize = std::mem::size_of::<PyObject>() as crate::abi_types::Py_ssize_t;
            object.tp_flags = crate::abi_types::Py_TPFLAGS_DEFAULT
                | crate::abi_types::Py_TPFLAGS_READY
                | inherited_flags;
            object.tp_base = base;
            ManagedView::Type {
                object: Box::new(UnsafeCell::new(object)),
                _name: name,
            }
        } else if tag == MoltTypeTag::Tuple {
            let len = unsafe { (crate::hooks::hooks_or_stubs().tuple_len)(bits) };
            let allocation = TupleAllocation::new(ob_refcnt, ob_type, len)?;
            ManagedView::Tuple { allocation }
        } else if tag == MoltTypeTag::List {
            let len = unsafe { (crate::hooks::hooks_or_stubs().list_len)(bits) };
            let allocation = ListAllocation::new(ob_refcnt, ob_type, len)?;
            ManagedView::List { allocation }
        } else if tag == MoltTypeTag::Exception {
            ManagedView::Exception(Box::new(UnsafeCell::new(PyBaseExceptionObject {
                ob_base: PyObject { ob_refcnt, ob_type },
                dict: std::ptr::null_mut(),
                args: std::ptr::null_mut(),
                notes: std::ptr::null_mut(),
                traceback: std::ptr::null_mut(),
                context: std::ptr::null_mut(),
                cause: std::ptr::null_mut(),
                suppress_context: 0,
            })))
        } else {
            ManagedView::Object(Box::new(BridgeHeader {
                py_obj: UnsafeCell::new(PyObject { ob_refcnt, ob_type }),
            }))
        };
        let entry = Box::new(BridgeEntry {
            view,
            bits,
            utf8: None,
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
                    PublicationState::Building { owner } if is_publication_owner(bits, owner) => {
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
            if let Some(addr) = handle.raw_py.get(&bits).copied() {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
                if increment {
                    unsafe { Self::increment_pyobj_ref(ptr) };
                }
                return Some(ptr);
            }
            return None;
        }
    }

    fn finish_publication(&self, bits: AbiHandle) {
        let index = self.handle_shard_index(bits);
        let mut handle = self.handle_shards[index].lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            eprintln!("molt fatal: ABI publication vanished before commit");
            std::process::abort();
        };
        match &entry.publication {
            PublicationState::Building { owner } if is_publication_owner(bits, owner) => {
                entry.publication = PublicationState::Ready;
            }
            state => {
                eprintln!("molt fatal: invalid ABI publication commit state: {state:?}");
                std::process::abort();
            }
        }
        self.publication_ready[index].notify_all();
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
        loop {
            if let Some(ptr) = unsafe { self.published_pyobj(bits, owned) } {
                if owned {
                    unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                }
                return ptr;
            }

            // A borrowed crossing must manufacture the stable runtime hold;
            // an owned crossing transfers its incoming hold to the view.
            if !owned {
                unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(bits) };
            }
            let has_non_view_runtime_owner = if owned {
                (unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) }) > 1
            } else {
                true
            };
            let initial_refs = if owned {
                1 + isize::from(has_non_view_runtime_owner)
            } else {
                1
            };
            let Some((entry, raw_ptr)) =
                (unsafe { self.build_pyobj_entry(bits, initial_refs, has_non_view_runtime_owner) })
            else {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                let pending = crate::api::errors::take_current_error();
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                drop(crate::api::errors::take_current_error());
                if let Some(pending) = pending {
                    crate::api::errors::restore_current_error_exact(pending);
                }
                unsafe {
                    ensure_result_error(c"managed runtime handle could not build an ABI view")
                };
                return std::ptr::null_mut();
            };
            let addr = raw_ptr.addr();
            let build_guard = PublicationBuildGuard::enter(bits);
            let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
            if handle.to_py.contains_key(&bits) || handle.raw_py.contains_key(&bits) {
                drop(handle);
                drop(address);
                drop(build_guard);
                if !owned {
                    unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                }
                continue;
            }
            if address.from_py.try_reserve(1).is_err() || handle.to_py.try_reserve(1).is_err() {
                drop(handle);
                drop(address);
                drop(build_guard);
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return std::ptr::null_mut();
            }
            if unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 1) } == 0 {
                drop(handle);
                drop(address);
                drop(build_guard);
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_RuntimeError).cast::<PyObject>(),
                        c"cannot publish ABI view for deallocating object".as_ptr(),
                    )
                };
                return std::ptr::null_mut();
            }
            address.from_py.insert(addr, bits);
            handle.to_py.insert(bits, entry);
            drop(handle);
            drop(address);

            if !self.refresh_tuple_view(bits)
                || !self.refresh_list_view(bits)
                || !self.refresh_exception_view(bits)
            {
                self.remove_managed_view(bits, addr);
                drop(build_guard);
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                unsafe {
                    ensure_result_error(c"managed runtime handle could not populate an ABI view")
                };
                return std::ptr::null_mut();
            }
            self.finish_publication(bits);
            drop(build_guard);
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

    pub unsafe fn owned_result_to_pyobj(
        &self,
        result: crate::hooks::OwnedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::transfer_runtime_pending_to_current() {
                    let pending = crate::api::errors::take_current_error();
                    unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                    drop(crate::api::errors::take_current_error());
                    if let Some(pending) = pending {
                        crate::api::errors::restore_current_error_exact(pending);
                    }
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
                if crate::api::errors::transfer_runtime_pending_to_current() {
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
                if crate::api::errors::transfer_runtime_pending_to_current() {
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime borrowed-result hook returned a value with an exception set",
                        )
                    };
                    return std::ptr::null_mut();
                }
                let ptr = unsafe { self.handle_to_borrowed_pyobj(bits) };
                if ptr.is_null() {
                    unsafe {
                        ensure_result_error(
                            c"runtime borrowed-result handle could not enter the bridge",
                        )
                    };
                    return std::ptr::null_mut();
                }
                unsafe { crate::api::refcount::Py_INCREF(ptr) };
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
            let visible = handle.raw_py.get(&bits).copied() == Some(addr)
                || handle.to_py.get(&bits).is_some_and(|entry| {
                    entry.view.py_obj().addr() == addr
                        && (matches!(entry.publication, PublicationState::Ready)
                            || matches!(
                                &entry.publication,
                                PublicationState::Building { owner }
                                    if is_publication_owner(bits, owner)
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

// Physical tuple/list projection preparation and publication.
impl ObjectBridge {
    pub fn unicode_utf8_cache(&self, bits: AbiHandle, bytes: &[u8]) -> Option<(*const u8, usize)> {
        let mut handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get_mut(&bits)?;
        let cache = entry.utf8.get_or_insert_with(|| {
            let mut nul_terminated = Vec::with_capacity(bytes.len() + 1);
            nul_terminated.extend_from_slice(bytes);
            nul_terminated.push(0);
            nul_terminated.into_boxed_slice()
        });
        Some((cache.as_ptr(), cache.len() - 1))
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
                crate::hooks::DecodedHandleResult::Ok(item_bits) if item_bits != 0 => {
                    let Some(pointer) = self.list_projection_pointer(item_bits) else {
                        for pointer in staged {
                            unsafe { self.projection_decref(pointer) };
                        }
                        return false;
                    };
                    pointer
                }
                crate::hooks::DecodedHandleResult::Ok(_)
                | crate::hooks::DecodedHandleResult::Missing => std::ptr::null_mut(),
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
        if pointer.is_null() || !valid_slot || !self.pyobj_matches_handle(pointer, value_bits) {
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
        if pointer.is_null() {
            return false;
        }
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
            if !allocation.initialized[index] {
                allocation.uninitialized_count = allocation.uninitialized_count.saturating_sub(1);
                allocation.initialized[index] = true;
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
            if !crate::api::errors::transfer_runtime_pending_to_current() {
                unsafe { ensure_result_error(c"runtime list snapshot commit failed") };
            }
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

    /// Snapshot list projection edges for cycle-GC traversal.
    pub fn list_view_handles_for_gc(&self, bits: AbiHandle) -> Vec<AbiHandle> {
        let fields = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return Vec::new();
            };
            let ManagedView::List { allocation } = &entry.view else {
                return Vec::new();
            };
            allocation
                .items
                .iter()
                .copied()
                .zip(allocation.shadow.iter().copied())
                // Clean projection references duplicate canonical runtime list
                // edges and are excluded from GC roots. Only a dirty direct C
                // slot is an additional internal edge that must be traversed
                // until commit adopts it into `shadow`.
                .filter_map(|(current, shadow)| {
                    (current != shadow && !current.is_null()).then_some(current)
                })
                .collect::<Vec<_>>()
        };
        fields
            .into_iter()
            .filter_map(|field| self.managed_handle_for_pyobj(field))
            .collect()
    }

    /// Publish an empty list projection before releasing its C ownership edges.
    pub fn clear_list_view(&self, bits: AbiHandle) {
        let (old, direct) = {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return;
            };
            let ManagedView::List { allocation } = &mut entry.view else {
                return;
            };
            let direct = allocation
                .items
                .iter()
                .copied()
                .zip(allocation.shadow.iter().copied())
                .filter_map(|(current, shadow)| {
                    (current != shadow && !current.is_null()).then_some(current)
                })
                .collect::<Vec<_>>();
            let old = std::mem::take(&mut allocation.shadow);
            allocation.items.clear();
            allocation.shadow.clear();
            allocation.initialized.clear();
            allocation.uninitialized_count = 0;
            allocation.sealed = true;
            allocation.publish_storage();
            (old, direct)
        };
        for pointer in old.into_iter().filter(|pointer| !pointer.is_null()) {
            unsafe { self.projection_decref(pointer) };
        }
        for pointer in direct {
            unsafe { crate::api::refcount::Py_DECREF(pointer) };
        }
    }
}

// Physical exception projection refresh, GC traversal, direct-write commit, and clear.
impl ObjectBridge {
    /// Pull the complete runtime exception state into its physical
    /// `PyBaseExceptionObject` in one publication.  The hook pins every field;
    /// conversion happens before the bridge lock and old C references are
    /// released only after the atomic pointer swap.
    pub fn refresh_exception_view(&self, bits: AbiHandle) -> bool {
        let is_exception = {
            let handle = self.handle_shard(bits).lock();
            matches!(
                handle.to_py.get(&bits).map(|entry| &entry.view),
                Some(ManagedView::Exception(_))
            )
        };
        if !is_exception {
            return true;
        }
        let Some(_sync) = ExceptionSyncGuard::enter(bits) else {
            return true;
        };
        let hooks = crate::hooks::hooks_or_stubs();
        let mut snapshot = crate::hooks::ExceptionSnapshot::default();
        if unsafe { (hooks.exception_snapshot)(bits, &raw mut snapshot) } != 0 {
            if !crate::api::errors::transfer_runtime_pending_to_current() {
                unsafe { ensure_result_error(c"runtime exception snapshot failed") };
            }
            return false;
        }
        let masks = [
            crate::hooks::EXCEPTION_SNAPSHOT_DICT,
            crate::hooks::EXCEPTION_SNAPSHOT_ARGS,
            crate::hooks::EXCEPTION_SNAPSHOT_NOTES,
            crate::hooks::EXCEPTION_SNAPSHOT_TRACEBACK,
            crate::hooks::EXCEPTION_SNAPSHOT_CONTEXT,
            crate::hooks::EXCEPTION_SNAPSHOT_CAUSE,
        ];
        let mut handles = [
            snapshot.dict,
            snapshot.args,
            snapshot.notes,
            snapshot.traceback,
            snapshot.context,
            snapshot.cause,
        ];
        let known_mask = masks.into_iter().fold(0, |known, mask| known | mask);
        let malformed = snapshot.present_mask & !known_mask != 0
            || snapshot.present_mask & crate::hooks::EXCEPTION_SNAPSHOT_ARGS == 0
            || snapshot.suppress_context > 1
            || handles.iter().zip(masks).any(|(handle, mask)| {
                let present = snapshot.present_mask & mask != 0;
                present != (*handle != 0)
            });
        if malformed {
            for handle_bits in handles.into_iter().filter(|bits| *bits != 0) {
                unsafe { (hooks.dec_ref)(handle_bits) };
            }
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"runtime returned a malformed exception snapshot".as_ptr(),
                )
            };
            return false;
        }
        let mut fields: [*mut PyObject; 6] = [std::ptr::null_mut(); 6];
        for index in 0..handles.len() {
            if snapshot.present_mask & masks[index] == 0 {
                continue;
            }
            let handle_bits = std::mem::take(&mut handles[index]);
            let field = unsafe { self.owned_handle_to_pyobj(handle_bits) };
            if field.is_null() {
                for field in fields.into_iter().filter(|field| !field.is_null()) {
                    unsafe { crate::api::refcount::Py_DECREF(field) };
                }
                for handle_bits in handles.into_iter().filter(|bits| *bits != 0) {
                    unsafe { (hooks.dec_ref)(handle_bits) };
                }
                return false;
            }
            fields[index] = field;
        }
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            drop(handle);
            for field in fields.into_iter().filter(|field| !field.is_null()) {
                unsafe { crate::api::refcount::Py_DECREF(field) };
            }
            return false;
        };
        let ManagedView::Exception(object) = &mut entry.view else {
            drop(handle);
            for field in fields.into_iter().filter(|field| !field.is_null()) {
                unsafe { crate::api::refcount::Py_DECREF(field) };
            }
            return false;
        };
        let old = unsafe {
            let object = &mut *object.get();
            let old = [
                object.dict,
                object.args,
                object.notes,
                object.traceback,
                object.context,
                object.cause,
            ];
            object.dict = fields[0];
            object.args = fields[1];
            object.notes = fields[2];
            object.traceback = fields[3];
            object.context = fields[4];
            object.cause = fields[5];
            object.suppress_context = snapshot.suppress_context as std::os::raw::c_char;
            old
        };
        drop(handle);
        for field in old.into_iter().filter(|field| !field.is_null()) {
            unsafe { crate::api::refcount::Py_DECREF(field) };
        }
        true
    }

    /// Snapshot managed children owned by the physical
    /// `PyBaseExceptionObject` projection for cycle-GC traversal.
    ///
    /// These C references are real graph edges in addition to the runtime
    /// exception payload edges. Copy the raw pointers while holding only the
    /// parent handle lock, then resolve them after dropping it: a self edge may
    /// hash to the same shard, so nested resolution under the parent lock would
    /// deadlock.
    pub fn exception_view_handles_for_gc(&self, bits: AbiHandle) -> [AbiHandle; 6] {
        let fields = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return [0; 6];
            };
            let ManagedView::Exception(object) = &entry.view else {
                return [0; 6];
            };
            unsafe {
                let object = &*object.get();
                [
                    object.dict,
                    object.args,
                    object.notes,
                    object.traceback,
                    object.context,
                    object.cause,
                ]
            }
        };
        let mut handles = [0; 6];
        for (index, field) in fields.into_iter().enumerate() {
            if let Some(field_bits) = self.managed_handle_for_pyobj(field) {
                handles[index] = field_bits;
            }
        }
        handles
    }
}

// Exception projection clear and C-written snapshot commit.
impl ObjectBridge {
    /// Publish an empty physical exception projection, then release its six C
    /// ownership edges outside bridge locks. Runtime exception slots must be
    /// detached first so any decref-triggered callback observes one coherent
    /// cleared object. Null publication also makes later bridge-entry teardown
    /// idempotent instead of double-decrementing the old fields.
    pub fn clear_exception_view_fields(&self, bits: AbiHandle) {
        let old = {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return;
            };
            let ManagedView::Exception(object) = &mut entry.view else {
                return;
            };
            unsafe {
                let object = &mut *object.get();
                [
                    std::mem::replace(&mut object.dict, std::ptr::null_mut()),
                    std::mem::replace(&mut object.args, std::ptr::null_mut()),
                    std::mem::replace(&mut object.notes, std::ptr::null_mut()),
                    std::mem::replace(&mut object.traceback, std::ptr::null_mut()),
                    std::mem::replace(&mut object.context, std::ptr::null_mut()),
                    std::mem::replace(&mut object.cause, std::ptr::null_mut()),
                ]
            }
        };
        for field in old.into_iter().filter(|field| !field.is_null()) {
            unsafe { crate::api::refcount::Py_DECREF(field) };
        }
    }

    /// Commit direct C writes to a managed `PyBaseExceptionObject` before the
    /// runtime observes that exception again.  Every C pointer is temporarily
    /// pinned, converted to an owned runtime handle, validated as a complete
    /// snapshot, and only then published by the runtime hook.
    pub fn commit_exception_view(&self, bits: AbiHandle) -> bool {
        let Some(_sync) = ExceptionSyncGuard::enter(bits) else {
            return true;
        };
        let fields = loop {
            let snapshot = {
                let handle = self.handle_shard(bits).lock();
                let Some(entry) = handle.to_py.get(&bits) else {
                    return true;
                };
                let ManagedView::Exception(object) = &entry.view else {
                    return true;
                };
                unsafe {
                    let object = &*object.get();
                    (
                        [
                            object.dict,
                            object.args,
                            object.notes,
                            object.traceback,
                            object.context,
                            object.cause,
                        ],
                        object.suppress_context,
                    )
                }
            };
            for field in snapshot.0.into_iter().filter(|field| !field.is_null()) {
                unsafe { crate::api::refcount::Py_INCREF(field) };
            }
            let current = {
                let handle = self.handle_shard(bits).lock();
                handle.to_py.get(&bits).and_then(|entry| {
                    let ManagedView::Exception(object) = &entry.view else {
                        return None;
                    };
                    unsafe {
                        let object = &*object.get();
                        Some((
                            [
                                object.dict,
                                object.args,
                                object.notes,
                                object.traceback,
                                object.context,
                                object.cause,
                            ],
                            object.suppress_context,
                        ))
                    }
                })
            };
            if current == Some(snapshot) {
                break snapshot;
            }
            for field in snapshot.0.into_iter().filter(|field| !field.is_null()) {
                unsafe { crate::api::refcount::Py_DECREF(field) };
            }
            if current.is_none() {
                return true;
            }
        };
        let (c_fields, suppress_context) = fields;
        if c_fields[1].is_null() || !matches!(suppress_context, 0 | 1) {
            for field in c_fields.into_iter().filter(|field| !field.is_null()) {
                unsafe { crate::api::refcount::Py_DECREF(field) };
            }
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"invalid direct PyBaseExceptionObject field state".as_ptr(),
                )
            };
            return false;
        }
        let masks = [
            crate::hooks::EXCEPTION_SNAPSHOT_DICT,
            crate::hooks::EXCEPTION_SNAPSHOT_ARGS,
            crate::hooks::EXCEPTION_SNAPSHOT_NOTES,
            crate::hooks::EXCEPTION_SNAPSHOT_TRACEBACK,
            crate::hooks::EXCEPTION_SNAPSHOT_CONTEXT,
            crate::hooks::EXCEPTION_SNAPSHOT_CAUSE,
        ];
        let hooks = crate::hooks::hooks_or_stubs();
        let mut runtime_fields = [0u64; 6];
        let mut present_mask = 0u32;
        let mut converted = true;
        for (index, field) in c_fields.iter().copied().enumerate() {
            if field.is_null() {
                continue;
            }
            let Some(value_bits) = (unsafe { self.molt_value_for_pyobj(field) }) else {
                converted = false;
                break;
            };
            runtime_fields[index] = value_bits;
            present_mask |= masks[index];
        }
        let snapshot = crate::hooks::ExceptionSnapshot {
            present_mask,
            suppress_context: suppress_context as u32,
            dict: runtime_fields[0],
            args: runtime_fields[1],
            notes: runtime_fields[2],
            traceback: runtime_fields[3],
            context: runtime_fields[4],
            cause: runtime_fields[5],
        };
        let committed = converted
            && unsafe { (hooks.exception_commit_snapshot)(bits, &raw const snapshot) } == 0;
        for value_bits in runtime_fields.into_iter().filter(|bits| *bits != 0) {
            unsafe { (hooks.dec_ref)(value_bits) };
        }
        for field in c_fields.into_iter().filter(|field| !field.is_null()) {
            unsafe { crate::api::refcount::Py_DECREF(field) };
        }
        if !committed && !crate::api::errors::transfer_runtime_pending_to_current() {
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
        match Self::classify_handle(value.bits()) {
            MoltTypeTag::Exception if !self.commit_exception_view(value.bits()) => return None,
            MoltTypeTag::List if !self.commit_list_view(value.bits()) => return None,
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
                PublicationState::Building { owner } if is_publication_owner(bits, owner) => {
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
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            return Some(bits);
        }
        if let Some(value) = self.observed_handle_for_pyobj(ptr) {
            let hooks = crate::hooks::hooks_or_stubs();
            unsafe { (hooks.inc_ref)(value.bits()) };
            return Some(value.bits());
        }
        unsafe { self.foreign_wrapper_for(ptr) }
    }

    unsafe fn foreign_wrapper_for(&self, ptr: *mut PyObject) -> Option<u64> {
        let key = ptr.expose_provenance();
        let hooks = crate::hooks::hooks_or_stubs();
        let address_index = self.address_shard_index(key);
        let mut address = self.address_shards[address_index].lock();
        loop {
            if let Some(wrapper) = address.foreign.get(&key).copied() {
                drop(address);
                unsafe { (hooks.inc_ref)(wrapper) };
                return Some(wrapper);
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
            return None;
        }

        // Acquire the C custody edge before taking bridge publication locks.
        // Py_INCREF probes managed membership and therefore re-enters the
        // address shard; doing it under `lock_address_then_handle` deadlocks on
        // the first genuine foreign crossing.
        unsafe { crate::api::refcount::Py_INCREF(ptr) };
        let (mut address, mut handle) = self.lock_address_then_handle(key, wrapper);
        address.foreign.insert(key, wrapper);
        address.foreign_inflight.remove(&key);
        handle.raw_py.insert(wrapper, key);
        self.foreign_ready[address_index].notify_all();
        Some(wrapper)
    }

    pub unsafe fn release_foreign(&self, c_ptr: usize) {
        let mut address = self.address_shard(c_ptr).lock();
        let Some(wrapper) = address.foreign.get(&c_ptr).copied() else {
            return;
        };
        let mut handle = self.handle_shard(wrapper).lock();
        address.foreign.remove(&c_ptr);
        handle.raw_py.remove(&wrapper);
    }

    #[cfg(test)]
    pub(crate) fn insert_foreign_for_test(&self, ptr: *mut PyObject, handle_bits: AbiHandle) {
        let exposed_addr = ptr.expose_provenance();
        let (mut address, mut handle) = self.lock_address_then_handle(ptr.addr(), handle_bits);
        address.foreign.insert(ptr.addr(), handle_bits);
        handle.raw_py.insert(handle_bits, exposed_addr);
    }

    pub unsafe fn register_foreign_pyobj(&self, ptr: *mut PyObject) -> AbiHandle {
        // A foreign C pointer is represented only by a real TYPE_ID_FOREIGN
        // runtime object.  Synthetic 0xA11C raw identities were not decodable
        // Molt values and formed a second handle representation.
        unsafe { self.foreign_wrapper_for(ptr) }.unwrap_or(0)
    }

    /// Rebind a canonical static C object (notably `PyExc_*`) from its
    /// bootstrap binding to a real runtime handle. Ingress always
    /// uses `direct_molt_py`; when `canonical_view` is true, handle-to-PyObject
    /// projection also resolves to this immortal static pointer. Registration
    /// runs before extension execution, so no managed view may already exist
    /// for a canonical reverse binding.
    pub unsafe fn bind_static_pyobj_to_runtime_handle(
        &self,
        ptr: *mut PyObject,
        bits: AbiHandle,
        canonical_view: bool,
    ) -> bool {
        if ptr.is_null() || bits == 0 {
            return false;
        }
        if canonical_view {
            let handle = self.handle_shard(bits).lock();
            if handle.to_py.contains_key(&bits)
                || handle
                    .raw_py
                    .get(&bits)
                    .is_some_and(|addr| *addr != ptr.addr())
            {
                return false;
            }
        }
        let addr = ptr.addr();
        let old_bits = {
            let mut address = self.address_shard(addr).lock();
            let old = address.from_py.insert(addr, bits);
            address.direct_molt_py.insert(addr, bits);
            old
        };
        if let Some(old_bits) = old_bits
            && old_bits != bits
        {
            self.handle_shard(old_bits).lock().raw_py.remove(&old_bits);
        }
        if canonical_view {
            self.handle_shard(bits).lock().raw_py.insert(bits, addr);
        }
        true
    }

    pub unsafe fn register_pyobj_for_handle(&self, ptr: *mut PyObject, bits: AbiHandle) {
        if ptr.is_null() || bits == 0 {
            return;
        }
        let addr = ptr.expose_provenance();
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        address.from_py.insert(addr, bits);
        address.direct_molt_py.insert(addr, bits);
        handle.raw_py.insert(bits, addr);
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
    /// Add one C reference owned by a physical ABI projection. The separate
    /// ledger lets finalization distinguish graph edges traversed by cycle GC
    /// from external C roots that genuinely resurrect an object.
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

    /// Register an already-owned C reference (for example a freshly
    /// materialized numeric carrier) as a projection edge without incrementing
    /// it a second time.
    unsafe fn projection_adopt_owned_ref(&self, ptr: *mut PyObject) -> bool {
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            return true;
        }
        let mut address = self.address_shard(ptr.addr()).lock();
        if let Some(count) = address.projection_refs.get_mut(&ptr.addr()) {
            let Some(next) = count.checked_add(1) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return false;
            };
            *count = next;
            return true;
        }
        if address.projection_refs.try_reserve(1).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        }
        address.projection_refs.insert(ptr.addr(), 1);
        true
    }

    /// Remove one adopted projection edge without consuming the underlying C
    /// reference. Used only to unwind a pre-publication failure.
    unsafe fn projection_unadopt_owned_ref(&self, ptr: *mut PyObject) {
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            return;
        }
        let mut address = self.address_shard(ptr.addr()).lock();
        let remove = match address.projection_refs.get_mut(&ptr.addr()) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => true,
            None => {
                eprintln!("molt fatal: missing projection reference ledger entry");
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
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
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

// Managed-view C-reference linearization, publication retirement, and view removal.
impl ObjectBridge {
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
                PublicationState::Building { owner } if is_publication_owner(bits, owner) => {}
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
                PublicationState::Building { owner } if is_publication_owner(bits, owner) => {}
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
            if handle.raw_py.remove(&bits).is_some() {
                address.direct_molt_py.remove(&addr);
                address.from_py.remove(&addr);
                return PyObjRelease::DirectViewUnregistered;
            }
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return PyObjRelease::Untracked;
            };
            match &entry.publication {
                PublicationState::Ready => entry.publication = PublicationState::Retiring,
                PublicationState::Building { owner } if is_publication_owner(bits, owner) => {
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

    /// Retire tuple identity immediately while deferring the projection's C
    /// references. The runtime terminal path drops the returned guard only
    /// after releasing inline tuple-owned runtime edges, so a projected child
    /// cannot be finalized between the two independent ownership releases.
    pub fn retire_tuple_view_deferred(&self, bits: AbiHandle) -> Option<RetiredTupleView> {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            if !matches!(&entry.view, ManagedView::Tuple { .. }) {
                return None;
            }
            entry.view.py_obj().addr()
        };
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        handle.to_py.get_mut(&bits)?.publication = PublicationState::Retiring;
        let entry = handle.to_py.remove(&bits)?;
        self.publication_ready[self.handle_shard_index(bits)].notify_all();
        drop(handle);
        drop(address);
        Some(RetiredTupleView { entry: Some(entry) })
    }

    /// Called after a runtime decrement leaves only the view's strong hold.
    /// `Some(true)` means the internal C bias was the last C reference and the
    /// caller must consume the view hold as the final runtime reference.
    /// `Some(false)` retains the view for direct CPython C references.
    pub fn runtime_owner_dropped_to_view_hold(&self, bits: AbiHandle) -> Option<bool> {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        match entry.lifecycle {
            BridgeLifecycle::FinalizingPin => return Some(false),
            BridgeLifecycle::ViewHoldOnly => {
                eprintln!(
                    "molt fatal: canonical ABI view lost runtime-owner bias before owner drop"
                );
                std::process::abort();
            }
            BridgeLifecycle::RuntimeOwned => {}
        }
        let py_obj = entry.view.py_obj();
        let refs = unsafe { (*py_obj).ob_refcnt };
        let Some(remaining) = checked_c_refs_without_bias(refs, true) else {
            abort_refcount_invariant("runtime owner drop", refs, entry.lifecycle);
        };
        unsafe { (*py_obj).ob_refcnt = remaining };
        entry.lifecycle = BridgeLifecycle::ViewHoldOnly;
        if remaining != 0 {
            return Some(false);
        }
        drop(handle);
        drop(address);
        Some(true)
    }
}

// Runtime-owner transitions, finalization pins, GC adjustments, and zero-ref classification.
impl ObjectBridge {
    /// Resolve the runtime's last-reference transition while preserving the
    /// canonical view through finalization. Any still-attached runtime bias is
    /// detached first. `false` means direct C references retain the view hold;
    /// `true` means the caller may consume that hold and enter finalization.
    pub fn runtime_last_ref_dropped(&self, bits: AbiHandle) -> bool {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return true;
        };
        let py_obj = entry.view.py_obj();
        match entry.lifecycle {
            BridgeLifecycle::FinalizingPin => return false,
            BridgeLifecycle::RuntimeOwned => {
                let refs = unsafe { (*py_obj).ob_refcnt };
                let Some(remaining) = checked_c_refs_without_bias(refs, true) else {
                    abort_refcount_invariant("last runtime reference drop", refs, entry.lifecycle);
                };
                unsafe { (*py_obj).ob_refcnt = remaining };
                entry.lifecycle = BridgeLifecycle::ViewHoldOnly;
            }
            BridgeLifecycle::ViewHoldOnly => {}
        }
        unsafe { (*py_obj).ob_refcnt == 0 }
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
        let Some(direct_and_projection) = checked_c_refs_without_bias(refs, lifecycle.has_c_bias())
        else {
            abort_refcount_invariant("direct C reference query", refs, lifecycle);
        };
        let projection = self
            .address_shard(ptr.addr())
            .lock()
            .projection_refs
            .get(&ptr.addr())
            .copied()
            .unwrap_or(0);
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
            let refs = unsafe { (*entry.view.py_obj()).ob_refcnt };
            let Some(remaining) = checked_c_refs_without_bias(refs, true) else {
                abort_refcount_invariant("finalization pin release", refs, entry.lifecycle);
            };
            unsafe { (*entry.view.py_obj()).ob_refcnt = remaining };
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

    /// Attach the runtime-owner C bias on the view-hold-only -> externally
    /// runtime-owned transition. The runtime calls this before publishing the
    /// new non-view strong reference.
    pub fn runtime_owner_added_from_view_hold(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        match entry.lifecycle {
            BridgeLifecycle::RuntimeOwned | BridgeLifecycle::FinalizingPin => return,
            BridgeLifecycle::ViewHoldOnly => {}
        }
        let py_obj = entry.view.py_obj();
        unsafe {
            if crate::abi_types::is_immortal_refcnt((*py_obj).ob_refcnt) {
                eprintln!("molt fatal: managed canonical ABI view was made immortal");
                std::process::abort();
            }
            let refs = (*py_obj).ob_refcnt;
            let Some(with_bias) = checked_c_ref_increment(refs) else {
                abort_refcount_invariant("runtime owner add", refs, entry.lifecycle);
            };
            (*py_obj).ob_refcnt = with_bias;
        }
        entry.lifecycle = BridgeLifecycle::RuntimeOwned;
    }
}

// Finalization completion, resurrection, GC roots, and zero-ref disposition.
impl ObjectBridge {
    /// Adjustment from raw runtime refcount to cycle-GC external-root count.
    /// The runtime count includes the view's one strong hold, which is not an
    /// external root; direct C references are external roots and live only in
    /// the C-visible header count beyond the runtime-owner bias.
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
        let Some(direct_and_projection) =
            checked_c_refs_without_bias(c_refs, lifecycle.has_c_bias())
        else {
            abort_refcount_invariant("GC external-root adjustment", c_refs, lifecycle);
        };
        let projection = self
            .address_shard(ptr.addr())
            .lock()
            .projection_refs
            .get(&ptr.addr())
            .copied()
            .unwrap_or(0);
        let Ok(projection) = isize::try_from(projection) else {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        };
        let Some(direct_c_refs) = direct_and_projection.checked_sub(projection) else {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        };
        if direct_c_refs < 0 {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        }
        direct_c_refs - 1
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
        let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
        {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return CRefZero::ViewRetained;
            };
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
                return CRefZero::ViewRetained;
            }
            if runtime_refs > 1 {
                unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
                return CRefZero::ViewRetained;
            }
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
                value if value == MoltTypeTag::Int as u8 => MoltTypeTag::Int,
                value if value == MoltTypeTag::Complex as u8 => MoltTypeTag::Complex,
                value if value == MoltTypeTag::Str as u8 => MoltTypeTag::Str,
                value if value == MoltTypeTag::Bytes as u8 => MoltTypeTag::Bytes,
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

/// Semantic type identity for source-recompiled extension consumers. Generic
/// managed views keep an honest physical `MoltManaged_Type`; this resolver
/// reports the runtime value's builtin semantic type without false layout
/// stamping. Foreign objects and full-layout carriers return physical type.
pub(crate) unsafe fn semantic_type(ptr: *mut PyObject) -> *mut PyTypeObject {
    let Some(resolved) = resolve_pyobject(ptr) else {
        return std::ptr::null_mut();
    };
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
    let tag = unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(handle.bits()) };
    match tag {
        x if x == MoltTypeTag::Int as u8 => &raw mut crate::abi_types::PyLong_Type,
        x if x == MoltTypeTag::Complex as u8 => &raw mut crate::abi_types::PyComplex_Type,
        x if x == MoltTypeTag::Str as u8 => &raw mut crate::abi_types::PyUnicode_Type,
        x if x == MoltTypeTag::Bytes as u8 => &raw mut crate::abi_types::PyBytes_Type,
        x if x == MoltTypeTag::List as u8 => &raw mut crate::abi_types::PyList_Type,
        x if x == MoltTypeTag::Tuple as u8 => &raw mut crate::abi_types::PyTuple_Type,
        x if x == MoltTypeTag::Dict as u8 => &raw mut crate::abi_types::PyDict_Type,
        x if x == MoltTypeTag::Set as u8 => &raw mut crate::abi_types::PySet_Type,
        x if x == MoltTypeTag::FrozenSet as u8 => &raw mut crate::abi_types::PyFrozenSet_Type,
        x if x == MoltTypeTag::Module as u8 => &raw mut crate::abi_types::PyModule_Type,
        _ => &raw mut crate::abi_types::MoltManaged_Type,
    }
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
        unsafe { crate::abi_types::init_static_types() };
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

/// `getattr` on a foreign wrapper: route through the wrapped C object's own
/// `tp_getattro` (else CPython generic getattr). `name_bits` is a Molt string
/// handle. Returns the attribute value as an owned Molt handle, or 0 with the C
/// slot's exception left pending.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_getattr(c_ptr: usize, name_bits: u64) -> u64 {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return 0;
    }
    let name_obj = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(name_bits) };
    if name_obj.is_null() {
        return 0;
    }
    let result = unsafe { foreign_slot_getattr(obj, name_obj) };
    unsafe { crate::api::refcount::Py_DECREF(name_obj) };
    if result.is_null() {
        return 0;
    }
    let bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(result) };
    // `molt_value_for_pyobj` minted its own owned reference for the Molt caller;
    // release the reference the C slot returned to us.
    unsafe { crate::api::refcount::Py_DECREF(result) };
    bits.unwrap_or(0)
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

/// Is `c_ptr` a C type object? Detected identity-free by walking its metatype
/// (`ob_type`) chain for a `type`-named metatype — robust to a static
/// extension's `&PyType_Type` retargeting to a copy of `type`.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
unsafe fn foreign_obj_is_type(c_ptr: usize) -> bool {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return false;
    }
    let mut mt = unsafe { (*obj).ob_type };
    let mut steps = 0u32;
    while !mt.is_null() && steps < 32 {
        let n = unsafe { (*mt).tp_name };
        if !n.is_null() && unsafe { std::ffi::CStr::from_ptr(n) }.to_bytes() == b"type" {
            return true;
        }
        mt = unsafe { (*mt).tp_base };
        steps += 1;
    }
    false
}

/// Resolve a *type* object's `__name__` / `__qualname__` from its own `tp_name`,
/// stripping any module/qualifier prefix (up to and including the last '.') —
/// exactly what CPython's `type.__name__` getter does. Writes the resolved name
/// (no NUL) into `out[..cap]` and returns its byte length, or -1 when `c_ptr` is
/// not a type object or has no `tp_name`.
///
/// This is deliberately **hooks-free** (no `str_data`/`alloc_str`, no bridge
/// proxy round-trip): the runtime side owns the attribute name and allocates the
/// result string itself, so `Type.__name__` resolves regardless of which
/// split-runtime module the getattr lands in. Our static `PyType_Type` carries
/// no `__name__` getset, and a static extension's metaclass (numpy's
/// `_DTypeMeta`) can reach Molt with no usable getattro, so this direct path is
/// the honest, robust answer for the numpy `numpy.dtypes._add_dtype_helper`
/// `DType.__name__` frontier.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`; `out` valid for `cap` bytes.
pub unsafe fn molt_foreign_type_dunder_name(c_ptr: usize, out: *mut u8, cap: usize) -> isize {
    if !unsafe { foreign_obj_is_type(c_ptr) } {
        return -1;
    }
    let tp = c_ptr as *mut PyTypeObject;
    let name_ptr = unsafe { (*tp).tp_name };
    if name_ptr.is_null() {
        return -1;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(name_ptr) }.to_bytes();
    let short = match bytes.iter().rposition(|&b| b == b'.') {
        Some(dot) => &bytes[dot + 1..],
        None => bytes,
    };
    if short.len() <= cap && !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(short.as_ptr(), out, short.len()) };
    }
    short.len() as isize
}

unsafe fn foreign_slot_getattr(obj: *mut PyObject, name_obj: *mut PyObject) -> *mut PyObject {
    // Walk the metatype's `tp_base` chain to find the first `tp_getattro` slot,
    // replicating CPython slot inheritance at call time. A static extension's
    // metaclass (numpy's `_DTypeMeta`, whose `tp_base` is `type`) can reach this
    // path with a null `tp_getattro` — either because inherit-slots was skipped
    // or because its `&PyType_Type` retargeted to a copy without our
    // `type_getattro` — and the chain walk still finds `PyType_Type`'s slot,
    // so `DType.__name__` resolves.
    let mut tp = unsafe { (*obj).ob_type };
    let mut steps = 0u32;
    while !tp.is_null() && steps < 32 {
        if let Some(getattro) = unsafe { (*tp).tp_getattro } {
            return unsafe { getattro(obj, name_obj) };
        }
        tp = unsafe { (*tp).tp_base };
        steps += 1;
    }
    // No tp_getattro slot anywhere in the metatype chain: fall back to CPython's
    // generic instance getattr.
    unsafe { crate::api::object::PyObject_GenericGetAttr(obj, name_obj) }
}

/// `setattr` on a foreign wrapper: route through the wrapped C object's own
/// `tp_setattro`. `value_bits == 0` means delete the attribute. Returns 0 on
/// success, -1 with the C slot's exception left pending on failure.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_setattr(
    c_ptr: usize,
    name_bits: u64,
    value_bits: u64,
) -> std::os::raw::c_int {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return -1;
    }
    let (name_obj, value_obj) = {
        let bridge = &*GLOBAL_BRIDGE;
        let name_obj = unsafe { bridge.owned_handle_to_pyobj(name_bits) };
        let value_obj = if value_bits == 0 {
            std::ptr::null_mut()
        } else {
            unsafe { bridge.owned_handle_to_pyobj(value_bits) }
        };
        (name_obj, value_obj)
    };
    if name_obj.is_null() {
        return -1;
    }
    let tp = unsafe { (*obj).ob_type };
    let rc = if !tp.is_null() {
        if let Some(setattro) = unsafe { (*tp).tp_setattro } {
            unsafe { setattro(obj, name_obj, value_obj) }
        } else {
            -1
        }
    } else {
        -1
    };
    unsafe { crate::api::refcount::Py_DECREF(name_obj) };
    if !value_obj.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(value_obj) };
    }
    rc
}

/// Call a foreign wrapper through the wrapped C object's `tp_call`. Exact Molt
/// tuples already own one canonical packed C projection, so the call borrows
/// that identity directly instead of allocating and copying a second tuple.
/// `args_bits` is a Molt tuple handle (0 = no positional args); `kwargs_bits` a
/// Molt dict handle (0 = none). Returns the result as an owned Molt handle, or 0
/// with the C slot's exception left pending.
///
/// # Safety
/// `c_ptr` must be a live callable C-extension `PyObject*`.
pub unsafe fn molt_foreign_call(c_ptr: usize, args_bits: u64, kwargs_bits: u64) -> u64 {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return 0;
    }
    let args_obj = unsafe { tuple_view_from_molt(args_bits) };
    if args_obj.is_null() {
        return 0;
    }
    let kwargs_obj = if kwargs_bits == 0 {
        std::ptr::null_mut()
    } else {
        unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(kwargs_bits) }
    };
    let result = unsafe { crate::api::object::PyObject_Call(obj, args_obj, kwargs_obj) };
    unsafe { crate::api::refcount::Py_DECREF(args_obj) };
    if !kwargs_obj.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(kwargs_obj) };
    }
    if result.is_null() {
        return 0;
    }
    let bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(result) };
    unsafe { crate::api::refcount::Py_DECREF(result) };
    bits.unwrap_or(0)
}

/// Acquire a new C reference to the canonical packed projection of a runtime
/// tuple. `owned_handle_to_pyobj` consumes one runtime reference, so pin the
/// borrowed call argument first.
unsafe fn tuple_view_from_molt(args_bits: u64) -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    if args_bits == 0 {
        return unsafe { crate::api::sequences::PyTuple_New(0) };
    }
    unsafe { (h.inc_ref)(args_bits) };
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(args_bits) }
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

const PY_NONE_HASH_BITS: u64 = 0x0FCA_86420;

#[inline]
fn py_hash_from_unsigned_bits(bits: u64) -> isize {
    if std::mem::size_of::<isize>() >= 8 {
        bits as isize
    } else {
        bits as u32 as i32 as isize
    }
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
        // CPython: hash(n) == n for small ints.
        mo.as_int().unwrap_or(0) as isize
    } else if mo.is_float() {
        let f = mo.as_float().unwrap_or(0.0);
        unsafe { crate::api::numbers::_Py_HashDouble(std::ptr::null_mut(), f) }
    } else if mo.is_bool() {
        mo.as_bool().unwrap_or(false) as isize
    } else if mo.is_none() {
        py_hash_from_unsigned_bits(PY_NONE_HASH_BITS)
    } else {
        py_hash_from_unsigned_bits(bits)
    };
    // CPython contract: a real value never hashes to the -1 error sentinel.
    if h == -1 { -2 } else { h }
}

/// Produce a Python-style repr string for a NaN-boxed value.
pub(crate) fn molt_repr_string(bits: u64) -> Option<Vec<u8>> {
    let mo = MoltObject::from_bits(bits);

    if mo.is_none() {
        return Some(b"None".to_vec());
    }
    if mo.is_bool() {
        return Some(if mo.as_bool().unwrap_or(false) {
            b"True".to_vec()
        } else {
            b"False".to_vec()
        });
    }
    if mo.is_int() {
        let i = mo.as_int_unchecked();
        return Some(i.to_string().into_bytes());
    }
    if mo.is_float() {
        let f = mo.as_float().unwrap_or(f64::NAN);
        return format_float_repr(f);
    }
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(bits) };
        if tag == MoltTypeTag::Str as u8 {
            // Strings get quoted in repr.
            let mut len: usize = 0;
            let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
            if !ptr.is_null() && len > 0 {
                let s = unsafe { std::slice::from_raw_parts(ptr, len) };
                let mut out = Vec::with_capacity(len + 2);
                out.push(b'\'');
                out.extend_from_slice(s);
                out.push(b'\'');
                return Some(out);
            }
            return Some(b"''".to_vec());
        }
    }
    None
}

/// Produce a Python-style str() string for a NaN-boxed value.
pub(crate) fn molt_str_string(bits: u64) -> Option<Vec<u8>> {
    let mo = MoltObject::from_bits(bits);

    if mo.is_none() {
        return Some(b"None".to_vec());
    }
    if mo.is_bool() {
        return Some(if mo.as_bool().unwrap_or(false) {
            b"True".to_vec()
        } else {
            b"False".to_vec()
        });
    }
    if mo.is_int() {
        let i = mo.as_int_unchecked();
        return Some(i.to_string().into_bytes());
    }
    if mo.is_float() {
        let f = mo.as_float().unwrap_or(f64::NAN);
        return format_float_repr(f);
    }
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(bits) };
        if tag == MoltTypeTag::Str as u8 {
            // str() of a string returns the string itself (unquoted).
            let mut len: usize = 0;
            let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
            if !ptr.is_null() && len > 0 {
                return Some(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec());
            }
            return Some(Vec::new());
        }
    }
    None
}

/// Format a float for `repr(float)` / `str(float)` through the runtime's single
/// float-format authority (`molt-lang-runtime`'s `object::float_repr`), exposed
/// over the `float_repr` runtime hook.
///
/// The ABI MUST NOT reimplement float formatting: Rust's own `{f}` produces the
/// correct number of digits but breaks round-half-to-even ties differently from
/// CPython's `_Py_dg_dtoa` (e.g. `137839762462415.625` renders as `...62` in
/// CPython but `...63` in Rust `std`). Routing through the runtime authority
/// keeps native `repr(float)` and the C-API path byte-for-byte identical.
fn format_float_repr(f: f64) -> Option<Vec<u8>> {
    let Some(h) = crate::hooks::hooks() else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"runtime float repr authority is unavailable".as_ptr(),
            )
        };
        return None;
    };
    // Max CPython float repr is "-1.7976931348623157e+308" (24 bytes); 32 is a
    // comfortable ceiling that avoids a second call in practice.
    let mut buf = [0u8; 32];
    let len = unsafe { (h.float_repr)(f, buf.as_mut_ptr(), buf.len()) };
    if len == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"runtime float repr authority failed".as_ptr(),
            )
        };
        None
    } else if len <= buf.len() {
        let mut out = Vec::new();
        if out.try_reserve_exact(len).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        out.extend_from_slice(&buf[..len]);
        Some(out)
    } else {
        // Extremely defensive: authority reported a longer string than our
        // buffer. Re-run into an exactly-sized buffer.
        let mut big = Vec::new();
        if big.try_reserve_exact(len).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        big.resize(len, 0);
        let written = unsafe { (h.float_repr)(f, big.as_mut_ptr(), big.len()) };
        if written != len {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"runtime float repr authority returned an inconsistent length".as_ptr(),
                )
            };
            return None;
        }
        Some(big)
    }
}

#[cfg(test)]
mod bridge_handle_tests {
    use super::*;
    use crate::abi_types::PyUnicode_Type;

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
        let empty = TupleAllocation::new(1, std::ptr::null_mut(), 0)
            .expect("empty tuple projection allocation");
        assert!(crate::abi_types::is_immortal_refcnt(unsafe {
            (*empty.object.as_ptr()).ob_base.ob_base.ob_refcnt
        }));
    }

    /// Regression for the numpy `_multiarray_umath` "'str' object is not
    /// callable" frontier: `PyObject_Call` routes bridge-managed Molt callables
    /// to the runtime `object_call` hook via `molt_handle_for_pyobj`, which must
    /// return genuine Molt handles for minted proxies and MUST NOT hand a
    /// raw-registry synthetic handle (not valid `MoltObject` bits) to the
    /// runtime.
    #[test]
    fn molt_handle_for_pyobj_excludes_raw_registered_pointers() {
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
        assert_eq!(unsafe { bridge.register_foreign_pyobj(stray_ptr) }, 0);
        assert!(bridge.pyobj_to_handle(stray_ptr).is_none());
        assert_eq!(bridge.molt_handle_for_pyobj(stray_ptr), None);
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
    ) -> (*mut PyObject, PublicationBuildGuard) {
        let (entry, ptr) = unsafe { bridge.build_pyobj_entry(bits, 1, false) }.unwrap();
        let guard = PublicationBuildGuard::enter(bits);
        let (mut address, mut handle) = bridge.lock_address_then_handle(ptr.addr(), bits);
        address.from_py.insert(ptr.addr(), bits);
        handle.to_py.insert(bits, entry);
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
        bridge.finish_publication(bits);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(bits));
        drop(guard);
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
        assert!(bridge.remove_managed_view(bits, addr));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        drop(guard);
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
        let (tx, rx) = mpsc::channel();
        let releaser = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(releaser.release_pyobj(ptr)).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        bridge.finish_publication(bits);
        drop(guard);
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
        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
        unsafe { (*ptr).ob_refcnt = crate::abi_types::IMMORTAL_REFCNT };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        assert_eq!(
            unsafe { bridge.managed_decref_pyobj(ptr) },
            ManagedDecref::Immortal
        );
        assert_eq!(
            unsafe { (*ptr).ob_refcnt },
            crate::abi_types::IMMORTAL_REFCNT
        );
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }
}
