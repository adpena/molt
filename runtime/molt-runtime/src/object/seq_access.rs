//! Scoped access to generic sequence backing storage.
//!
//! This is the sole object-layer read authority. Higher runtime layers never
//! borrow a raw `Vec` from an object: immutable tuples use zero-lock scoped or
//! pinned reads, while mutable lists use the stable backing lock, a pinned
//! item, or a resource-accounted owned snapshot.
//! Pins are semantic owners: ordinary retirement leaves the selected error
//! visible to native deallocators. Explicit cleanup supplies its own boundary.

use crate::object::{HEADER_FLAG_CONTAINS_REFS, HEADER_FLAG_HAS_ABI_VIEW};
use crate::*;

pub(crate) struct PinnedSequenceSnapshot<'a, 'py> {
    py: &'a PyToken<'py>,
    values: crate::object::backing::TrackedVecOwner<u64>,
}

pub(crate) struct PinnedSequenceItem<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}

impl<'a, 'py> PinnedSequenceItem<'a, 'py> {
    /// Adopt an item already retained by the physical sequence owner.
    /// # Safety
    /// `bits` transfers one live runtime reference under `py` custody.
    #[inline]
    pub(crate) unsafe fn from_owned_bits(py: &'a PyToken<'py>, bits: u64) -> Self {
        Self { py, bits }
    }

    #[inline]
    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }

    /// Transfer the pinned reference to an FFI caller.
    #[inline]
    pub(crate) fn into_bits(self) -> u64 {
        let bits = self.bits;
        std::mem::forget(self);
        bits
    }
}

impl Drop for PinnedSequenceItem<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

impl<'a, 'py> PinnedSequenceSnapshot<'a, 'py> {
    /// Adopt already-retained values from another object representation.
    pub(crate) fn from_owned_values(
        py: &'a PyToken<'py>,
        values: crate::object::backing::TrackedVecOwner<u64>,
    ) -> Self {
        Self { py, values }
    }

    /// Move retained elements and their accounted backing into a sequence.
    /// No reference is consumed unless the destination has been allocated.
    pub(crate) fn into_owned_values(self) -> crate::object::backing::TrackedVecOwner<u64> {
        let snapshot = std::mem::ManuallyDrop::new(self);
        unsafe { std::ptr::read(&snapshot.values) }
    }
}

impl std::ops::Deref for PinnedSequenceSnapshot<'_, '_> {
    type Target = [u64];

    fn deref(&self) -> &Self::Target {
        self.values.as_slice()
    }
}

impl Drop for PinnedSequenceSnapshot<'_, '_> {
    fn drop(&mut self) {
        for &bits in self.values.iter() {
            dec_ref_bits(self.py, bits);
        }
    }
}

#[inline]
unsafe fn with_locked_sequence_slice<R>(
    ptr: *mut u8,
    read: impl for<'slice> FnOnce(&'slice [u64]) -> R,
) -> R {
    let values = unsafe { crate::object::layout::seq_vec_ptr(ptr) };
    let read_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(values) };
    let result = read(unsafe { (&*values).as_slice() });
    drop(read_guard);
    result
}

#[inline]
unsafe fn tuple_slice<'a>(ptr: *mut u8) -> &'a [u64] {
    let len = unsafe { crate::object::layout::tuple_storage_len(ptr) };
    unsafe { std::slice::from_raw_parts(crate::object::layout::tuple_storage_items(ptr), len) }
}

#[inline]
unsafe fn tuple_slice_mut<'a>(ptr: *mut u8) -> &'a mut [u64] {
    let len = unsafe { crate::object::layout::tuple_storage_len(ptr) };
    unsafe {
        std::slice::from_raw_parts_mut(crate::object::layout::tuple_storage_items_mut(ptr), len)
    }
}

pub(crate) unsafe fn snapshot<'a, 'py>(
    py: &'a PyToken<'py>,
    ptr: *mut u8,
    failure_message: &str,
) -> Option<PinnedSequenceSnapshot<'a, 'py>> {
    if ptr.is_null() {
        return None;
    }
    let copy = |live: &[u64]| {
        let values = crate::object::backing::tracked_vec_box_from_slice(live, live.len())?;
        unsafe {
            for &bits in &*values {
                inc_ref_bits(py, bits);
            }
        }
        Some(values)
    };
    let values = if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        copy(unsafe { tuple_slice(ptr) })
    } else {
        unsafe { with_locked_sequence_slice(ptr, copy) }
    };
    let Some(values) = values else {
        let _ = raise_exception::<u64>(py, "MemoryError", failure_message);
        return None;
    };
    Some(PinnedSequenceSnapshot {
        py,
        values: unsafe { crate::object::backing::tracked_vec_box_from_raw(values) },
    })
}

/// Capture two sequences into one pinned, resource-accounted allocation.
/// Locks are acquired by backing address, giving free-threaded concatenation a
/// deterministic linearization point without the three allocations of two
/// independent snapshots plus a combined buffer.
pub(crate) unsafe fn snapshot_concat<'a, 'py>(
    py: &'a PyToken<'py>,
    lhs: *mut u8,
    rhs: *mut u8,
    failure_message: &str,
) -> Option<PinnedSequenceSnapshot<'a, 'py>> {
    if lhs.is_null() || rhs.is_null() {
        return None;
    }
    let copy = |left: &[u64], right: &[u64]| {
        let total = left.len().checked_add(right.len())?;
        let values = crate::object::backing::tracked_vec_box_with_capacity::<u64>(total)?;
        unsafe {
            (*values).extend_from_slice(left);
            (*values).extend_from_slice(right);
            for &bits in &*values {
                inc_ref_bits(py, bits);
            }
        }
        Some(values)
    };
    let lhs_is_tuple = unsafe { object_type_id(lhs) } == TYPE_ID_TUPLE;
    let rhs_is_tuple = unsafe { object_type_id(rhs) } == TYPE_ID_TUPLE;
    let values = if lhs_is_tuple && rhs_is_tuple {
        copy(unsafe { tuple_slice(lhs) }, unsafe { tuple_slice(rhs) })
    } else if lhs_is_tuple {
        let rhs_values = unsafe { crate::object::layout::seq_vec_ptr(rhs) };
        let guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(rhs_values) };
        let values = copy(unsafe { tuple_slice(lhs) }, unsafe {
            (&*rhs_values).as_slice()
        });
        drop(guard);
        values
    } else if rhs_is_tuple {
        let lhs_values = unsafe { crate::object::layout::seq_vec_ptr(lhs) };
        let guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(lhs_values) };
        let values = copy(unsafe { (&*lhs_values).as_slice() }, unsafe {
            tuple_slice(rhs)
        });
        drop(guard);
        values
    } else {
        let lhs_values = unsafe { crate::object::layout::seq_vec_ptr(lhs) };
        let rhs_values = unsafe { crate::object::layout::seq_vec_ptr(rhs) };
        if lhs_values == rhs_values {
            let guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(lhs_values) };
            let live = unsafe { (&*lhs_values).as_slice() };
            let values = copy(live, live);
            drop(guard);
            values
        } else if (lhs_values as usize) < (rhs_values as usize) {
            let lhs_guard =
                unsafe { crate::object::backing::tracked_vec_mutation_lock(lhs_values) };
            let rhs_guard =
                unsafe { crate::object::backing::tracked_vec_mutation_lock(rhs_values) };
            let values = copy(unsafe { (&*lhs_values).as_slice() }, unsafe {
                (&*rhs_values).as_slice()
            });
            drop(rhs_guard);
            drop(lhs_guard);
            values
        } else {
            let rhs_guard =
                unsafe { crate::object::backing::tracked_vec_mutation_lock(rhs_values) };
            let lhs_guard =
                unsafe { crate::object::backing::tracked_vec_mutation_lock(lhs_values) };
            let values = copy(unsafe { (&*lhs_values).as_slice() }, unsafe {
                (&*rhs_values).as_slice()
            });
            drop(lhs_guard);
            drop(rhs_guard);
            values
        }
    };
    let Some(values) = values else {
        let _ = raise_exception::<u64>(py, "MemoryError", failure_message);
        return None;
    };
    Some(PinnedSequenceSnapshot {
        py,
        values: unsafe { crate::object::backing::tracked_vec_box_from_raw(values) },
    })
}

pub(crate) unsafe fn pin_item<'a, 'py>(
    py: &'a PyToken<'py>,
    ptr: *mut u8,
    index: usize,
) -> Option<PinnedSequenceItem<'a, 'py>> {
    if ptr.is_null() {
        return None;
    }
    let pin = |items: &[u64]| {
        let bits = items.get(index).copied();
        if let Some(bits) = bits {
            inc_ref_bits(py, bits);
        }
        bits
    };
    let bits = if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        pin(unsafe { tuple_slice(ptr) })
    } else {
        unsafe { with_locked_sequence_slice(ptr, pin) }
    };
    bits.map(|bits| PinnedSequenceItem { py, bits })
}

#[inline]
pub(crate) unsafe fn locked_len(ptr: *mut u8) -> usize {
    if ptr.is_null() {
        return 0;
    }
    if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        return unsafe { tuple_slice(ptr).len() };
    }
    unsafe { with_locked_sequence_slice(ptr, |items| items.len()) }
}

pub(crate) unsafe fn with_immutable_tuple_slice<R>(
    ptr: *mut u8,
    read: impl for<'slice> FnOnce(&'slice [u64]) -> R,
) -> Option<R> {
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    Some(read(unsafe { tuple_slice(ptr) }))
}

/// A refcount-pinned immutable tuple view whose slice cannot outlive its owner.
pub(crate) struct PinnedTuple<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
    ptr: *mut u8,
}

impl std::ops::Deref for PinnedTuple<'_, '_> {
    type Target = [u64];

    fn deref(&self) -> &Self::Target {
        unsafe { tuple_slice(self.ptr) }
    }
}

impl Drop for PinnedTuple<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

pub(crate) unsafe fn pin_tuple<'a, 'py>(
    py: &'a PyToken<'py>,
    ptr: *mut u8,
) -> Option<PinnedTuple<'a, 'py>> {
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    inc_ref_bits(py, bits);
    Some(PinnedTuple { py, bits, ptr })
}

/// A stable sequence read using the existing owner for its storage kind.
/// Tuples retain their immutable inline storage in O(1); lists retain a
/// resource-accounted snapshot captured under the backing lock.
pub(crate) enum PinnedSequence<'a, 'py> {
    Tuple(PinnedTuple<'a, 'py>),
    Snapshot(PinnedSequenceSnapshot<'a, 'py>),
}

impl std::ops::Deref for PinnedSequence<'_, '_> {
    type Target = [u64];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Tuple(tuple) => tuple,
            Self::Snapshot(snapshot) => snapshot,
        }
    }
}

/// Select stable read custody without copying an immutable tuple. Unsupported
/// storage returns None; denied list snapshot allocation leaves MemoryError.
/// The tuple pin also prevents checked C-ABI and runtime unique-slot mutation.
/// Unchecked PyTuple_SET_ITEM retains its caller's new-tuple-only obligation.
pub(crate) unsafe fn pin_sequence<'a, 'py>(
    py: &'a PyToken<'py>,
    ptr: *mut u8,
    failure_message: &str,
) -> Option<PinnedSequence<'a, 'py>> {
    if ptr.is_null() {
        return None;
    }
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_TUPLE => unsafe { pin_tuple(py, ptr) }.map(PinnedSequence::Tuple),
        TYPE_ID_LIST => unsafe { snapshot(py, ptr, failure_message) }.map(PinnedSequence::Snapshot),
        _ => None,
    }
}

#[inline]
pub(crate) unsafe fn len(ptr: *mut u8) -> usize {
    unsafe { locked_len(ptr) }
}

#[inline]
pub(crate) unsafe fn item(ptr: *mut u8, index: usize) -> Option<u64> {
    if ptr.is_null() {
        return None;
    }
    if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        return unsafe {
            with_immutable_tuple_slice(ptr, |items| items.get(index).copied()).flatten()
        };
    }
    crate::gil_assert();
    unsafe { with_locked_sequence_slice(ptr, |items| items.get(index).copied()) }
}

/// Read a borrowed initialized tuple construction slot.
///
/// Physical storage uses the runtime's missing sentinel until a construction
/// slot is filled. C-API readers must distinguish that absence from every
/// legitimate object bit pattern, including float +0.0 (`Some(0)`) and None.
#[inline]
pub(crate) unsafe fn initialized_tuple_item(
    py: &PyToken<'_>,
    ptr: *mut u8,
    index: usize,
) -> Option<u64> {
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    unsafe { item(ptr, index) }.filter(|bits| *bits != missing_bits(py))
}

#[inline]
pub(crate) unsafe fn read_item_gil_borrowed(ptr: *mut u8, index: usize, out: *mut u64) -> i32 {
    crate::gil_assert();
    if out.is_null() {
        return 0;
    }
    let Some(bits) = (unsafe { item(ptr, index) }) else {
        return 0;
    };
    unsafe {
        *out = bits;
    }
    1
}

#[inline]
pub(crate) fn read_item_owned(ptr: *mut u8, index: usize, out: *mut u64) -> i32 {
    if ptr.is_null() || out.is_null() {
        return 0;
    }
    with_gil(|py| {
        let Some(item) = (unsafe { pin_item(&py, ptr, index) }) else {
            return 0;
        };
        unsafe {
            *out = item.into_bits();
        }
        1
    })
}

#[inline]
pub(crate) unsafe fn tuple_pair(ptr: *mut u8) -> Option<(u64, u64)> {
    unsafe {
        with_immutable_tuple_slice(ptr, |items| {
            (items.len() >= 2).then(|| (items[0], items[1]))
        })
        .flatten()
    }
}

#[cfg(test)]
#[inline]
pub(crate) unsafe fn tracked_heap_edge_count(ptr: *mut u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }
    if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        return Some(
            unsafe { tuple_slice(ptr) }
                .iter()
                .copied()
                .filter(|bits| crate::object::refcount_opt::is_heap_ref(*bits))
                .count(),
        );
    }
    let values = unsafe { crate::object::layout::seq_vec_ptr(ptr) };
    let read_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(values) };
    let count = unsafe { crate::object::backing::tracked_vec_heap_edge_count(values) };
    drop(read_guard);
    Some(count)
}

#[inline]
pub(crate) unsafe fn backing_identity(ptr: *mut u8) -> usize {
    if ptr.is_null() {
        0
    } else if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        unsafe { crate::object::layout::tuple_storage_items(ptr) as usize }
    } else {
        unsafe { crate::object::layout::seq_vec_ptr(ptr) as usize }
    }
}

#[inline]
pub(crate) unsafe fn with_borrowed<R>(
    ptr: *mut u8,
    read: impl for<'slice> FnOnce(&'slice [u64]) -> R,
) -> R {
    crate::gil_assert();
    if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
        return unsafe {
            with_immutable_tuple_slice(ptr, read)
                .expect("type-checked tuple must expose immutable storage")
        };
    }
    unsafe { with_locked_sequence_slice(ptr, read) }
}

#[inline]
unsafe fn adjust_tuple_contains_refs(ptr: *mut u8, removed: &[u64], added: &[u64]) {
    let added_ref = added
        .iter()
        .copied()
        .any(crate::object::refcount_opt::is_heap_ref);
    let removed_ref = removed
        .iter()
        .copied()
        .any(crate::object::refcount_opt::is_heap_ref);
    let header = unsafe { header_from_obj_ptr(ptr) };
    unsafe {
        if added_ref {
            (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        } else if removed_ref
            // Native subtype fields and class custody are sticky references;
            // a visible tuple edit cannot clear their GC admission.
            && !super::native_instance::has_fields(ptr)
            && !tuple_slice(ptr)
                .iter()
                .copied()
                .any(crate::object::refcount_opt::is_heap_ref)
        {
            (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
    }
}

/// Replace a fixed tuple slot admitted by the physical C-ABI owner.
///
/// PyTuple_SetItem enforces exclusive publication; PyTuple_SET_ITEM gives that
/// obligation to its unsafe caller. Both enter this one storage primitive.
/// The borrowed input gains a tuple edge, and the displaced owned edge is
/// returned for retirement after physical/runtime publication.
pub(crate) unsafe fn replace_capi_item(
    py: &PyToken<'_>,
    ptr: *mut u8,
    index: usize,
    value_bits: u64,
) -> Option<u64> {
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    let items = unsafe { tuple_slice_mut(ptr) };
    let slot = items.get_mut(index)?;
    inc_ref_bits(py, value_bits);
    let old_bits = std::mem::replace(slot, value_bits);
    unsafe { adjust_tuple_contains_refs(ptr, &[old_bits], &[value_bits]) };
    Some(old_bits)
}

/// One exclusive-owner admission for C-API initialization and runtime caches.
/// C-API writes deliberately admit their own physical ABI view; cache reuse adds
/// an export barrier below because a C-visible result must remain immutable.
unsafe fn tuple_is_exclusive(ptr: *mut u8) -> bool {
    !ptr.is_null()
        && unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE
        && unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() } == 1
}

/// Store one already-owned value into an exclusively-owned tuple slot.
/// Ownership of `value_bits` transfers on success; no refcount round trip is
/// performed.
pub(crate) unsafe fn replace_unique_item_owned(
    ptr: *mut u8,
    index: usize,
    value_bits: u64,
) -> Option<u64> {
    if !unsafe { tuple_is_exclusive(ptr) } {
        return None;
    }
    let items = unsafe { tuple_slice_mut(ptr) };
    let slot = items.get_mut(index)?;
    let old_bits = std::mem::replace(slot, value_bits);
    unsafe { adjust_tuple_contains_refs(ptr, &[old_bits], &[value_bits]) };
    Some(old_bits)
}

/// Admission shared by fixed pairs and variable-length iterator result caches.
/// A C-visible or aliased tuple remains immutable. The claim pins the result
/// before any callback; reentry consequently cannot claim that same cache.
unsafe fn tuple_cache_mutation_allowed(ptr: *mut u8, len: usize) -> bool {
    unsafe {
        tuple_is_exclusive(ptr)
            && (*header_from_obj_ptr(ptr)).load_synchronized_flags() & HEADER_FLAG_HAS_ABI_VIEW == 0
            && tuple_slice(ptr).len() == len
    }
}

/// A scoped mutation claim on an existing owned tuple. The initial edge count
/// is O(arity); each published slot replacement is O(1), including flag updates.
/// Counting once avoids rescanning the result after every removed heap edge.
pub(crate) struct TupleCacheMutation<'a, 'py> {
    py: &'a PyToken<'py>,
    owner: molt_runtime_core::OwnedRuntimeValue<'a>,
    heap_edges: usize,
}

impl<'a, 'py> TupleCacheMutation<'a, 'py> {
    pub(crate) unsafe fn claim(py: &'a PyToken<'py>, ptr: *mut u8, len: usize) -> Option<Self> {
        if !unsafe { tuple_cache_mutation_allowed(ptr, len) } {
            return None;
        }
        Some(Self {
            py,
            owner: molt_runtime_core::OwnedRuntimeValue::retain(
                py.core_token(),
                MoltObject::from_ptr(ptr).bits(),
            ),
            heap_edges: unsafe {
                tuple_slice(ptr)
                    .iter()
                    .copied()
                    .filter(|bits| crate::object::refcount_opt::is_heap_ref(*bits))
                    .count()
            },
        })
    }

    pub(crate) unsafe fn from_owned(
        py: &'a PyToken<'py>,
        owner: molt_runtime_core::OwnedRuntimeValue<'a>,
        len: usize,
    ) -> Option<Self> {
        let ptr = obj_from_bits(owner.bits()).as_ptr()?;
        if !unsafe { tuple_cache_mutation_allowed(ptr, len) } {
            return None;
        }
        let heap_edges = unsafe {
            tuple_slice(ptr)
                .iter()
                .copied()
                .filter(|bits| crate::object::refcount_opt::is_heap_ref(*bits))
                .count()
        };
        Some(Self {
            py,
            owner,
            heap_edges,
        })
    }

    /// Transfer an item edge; retire the returned old edge only after mutation
    /// is published. No payload borrow survives into Python callbacks.
    pub(crate) fn replace_owned(&mut self, index: usize, value: u64) -> u64 {
        let ptr = obj_from_bits(self.owner.bits())
            .as_ptr()
            .expect("tuple mutation owner");
        unsafe {
            let old = std::mem::replace(&mut tuple_slice_mut(ptr)[index], value);
            self.heap_edges -= usize::from(crate::object::refcount_opt::is_heap_ref(old));
            self.heap_edges += usize::from(crate::object::refcount_opt::is_heap_ref(value));
            let header = header_from_obj_ptr(ptr);
            if self.heap_edges != 0 {
                (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
                if !super::gc::gc_is_tracked(ptr)
                    && super::heap_lifecycle::value_requires_tracking(value)
                {
                    // A prior collection may have demoted the cache while it
                    // held only atomic values. Publish membership before an
                    // old edge's finalizer can collect the newly formed cycle.
                    super::gc::gc_track_if_cyclic(self.py, ptr, TYPE_ID_TUPLE);
                }
            } else if !super::native_instance::has_fields(ptr) {
                (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
            }
            old
        }
    }

    pub(crate) fn replace_borrowed(&mut self, index: usize, value: u64) -> u64 {
        inc_ref_bits(self.py, value);
        self.replace_owned(index, value)
    }

    pub(crate) fn into_bits(self) -> u64 {
        self.owner.into_bits()
    }
}

pub(crate) unsafe fn detach_tuple_edges(ptr: *mut u8, mut detach: impl FnMut(u64)) {
    let none = MoltObject::none().bits();
    // Tuple storage unwinds last-to-first, including finalizer reentry.
    for slot in unsafe { tuple_slice_mut(ptr) }.iter_mut().rev() {
        detach(std::mem::replace(slot, none));
    }
}
