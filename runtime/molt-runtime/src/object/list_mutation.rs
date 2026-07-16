use crate::object::{HEADER_FLAG_CONTAINS_REFS, HEADER_FLAG_HAS_ABI_VIEW};
use crate::*;
use molt_obj_model::MoltObject;

/// One transactional mutation of a published generic list that already owns a
/// CPython ABI view. The future runtime storage is fully tracked and refcounted
/// off to the side; its physical PyListObject projection is staged before the
/// live storage pointer changes. Commit is allocation-free and publishes both
/// authorities before releasing any displaced edge, so finalizers may re-enter
/// without observing a split state.
pub(crate) struct ListMutationTxn<'a, 'py> {
    py: &'a PyToken<'py>,
    ptr: *mut u8,
    bits: u64,
    base: *mut Vec<u64>,
    base_epoch: u64,
    next: *mut Vec<u64>,
    detached: Vec<u64>,
    committed: bool,
}

impl<'a, 'py> ListMutationTxn<'a, 'py> {
    /// # Safety
    /// `ptr` must be a live TYPE_ID_LIST object and the caller must hold the
    /// runtime GIL. Dirty C construction state must already have crossed the
    /// bridge's observed-ingress commit boundary.
    pub(crate) unsafe fn begin(py: &'a PyToken<'py>, ptr: *mut u8) -> Option<Self> {
        crate::gil_assert();
        if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return None;
        }
        let header = unsafe { header_from_obj_ptr(ptr) };
        if unsafe { (*header).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW == 0 {
            return None;
        }
        let base = unsafe { seq_vec_ptr(ptr) };
        let read_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
        let base_epoch = unsafe { crate::object::backing::tracked_vec_mutation_epoch(base) };
        let current = unsafe { &*base };
        let Some(next) = crate::object::backing::tracked_vec_box_from_slice(current, current.len())
        else {
            drop(read_guard);
            let _ = raise_exception::<u64>(py, "MemoryError", "list allocation failed");
            return None;
        };
        unsafe {
            crate::object::backing::tracked_vec_set_heap_edge_count(
                next,
                crate::object::refcount_opt::slice_heap_ref_count(current),
            )
        };
        for &item in current {
            inc_ref_bits(py, item);
        }
        drop(read_guard);
        Some(Self {
            py,
            ptr,
            bits: MoltObject::from_ptr(ptr).bits(),
            base,
            base_epoch,
            next,
            detached: Vec::new(),
            committed: false,
        })
    }

    #[inline]
    fn next(&self) -> &Vec<u64> {
        unsafe { &*self.next }
    }

    #[inline]
    fn next_mut(&mut self) -> &mut Vec<u64> {
        unsafe { &mut *self.next }
    }

    pub(crate) fn set_indices(&mut self, indices: &[usize], items: &[u64]) -> bool {
        if indices.len() != items.len() || indices.iter().any(|&index| index >= self.next().len()) {
            return false;
        }
        if self.detached.try_reserve(indices.len()).is_err() {
            let _ = raise_exception::<u64>(self.py, "MemoryError", "list allocation failed");
            return false;
        }
        let mut removed_edges = 0usize;
        let mut added_edges = 0usize;
        for (&index, &item) in indices.iter().zip(items) {
            inc_ref_bits(self.py, item);
            let old = std::mem::replace(&mut self.next_mut()[index], item);
            removed_edges += usize::from(crate::object::refcount_opt::is_heap_ref(old));
            added_edges += usize::from(crate::object::refcount_opt::is_heap_ref(item));
            self.detached.push(old);
        }
        unsafe {
            crate::object::backing::tracked_vec_adjust_heap_edge_count(
                self.next,
                removed_edges,
                added_edges,
            )
        };
        true
    }

    pub(crate) fn remove_indices(&mut self, removal_order: &[usize]) -> bool {
        let mut remaining = self.next().len();
        for &index in removal_order {
            if index >= remaining {
                return false;
            }
            remaining -= 1;
        }
        if self.detached.try_reserve(removal_order.len()).is_err() {
            let _ = raise_exception::<u64>(self.py, "MemoryError", "list allocation failed");
            return false;
        }
        let mut removed_edges = 0usize;
        for &index in removal_order {
            let removed = self.next_mut().remove(index);
            removed_edges += usize::from(crate::object::refcount_opt::is_heap_ref(removed));
            self.detached.push(removed);
        }
        unsafe {
            crate::object::backing::tracked_vec_adjust_heap_edge_count(self.next, removed_edges, 0)
        };
        true
    }

    pub(crate) fn repeat(&mut self, count: usize) -> bool {
        if count <= 1 || self.next().is_empty() {
            return true;
        }
        let initial_len = self.next().len();
        let initial_edges =
            unsafe { crate::object::backing::tracked_vec_heap_edge_count(self.next) };
        let Some(total) = initial_len.checked_mul(count) else {
            let _ = raise_exception::<u64>(
                self.py,
                "OverflowError",
                "cannot fit 'int' into an index-sized integer",
            );
            return false;
        };
        if !unsafe {
            crate::object::backing::tracked_vec_reserve_or_raise(
                self.py,
                self.next,
                total,
                "list allocation failed",
            )
        } {
            return false;
        }
        for _ in 1..count {
            for index in 0..initial_len {
                let item = self.next()[index];
                inc_ref_bits(self.py, item);
                self.next_mut().push(item);
            }
        }
        unsafe {
            crate::object::backing::tracked_vec_adjust_heap_edge_count(
                self.next,
                0,
                initial_edges.saturating_mul(count - 1),
            )
        };
        true
    }

    pub(crate) fn replace_range(&mut self, low: usize, high: usize, items: &[u64]) -> bool {
        if low > high || high > self.next().len() {
            return false;
        }
        let removed_len = high - low;
        let required = self
            .next()
            .len()
            .saturating_sub(removed_len)
            .saturating_add(items.len());
        let mut removed = Vec::new();
        if removed.try_reserve_exact(removed_len).is_err()
            || !unsafe {
                crate::object::backing::tracked_vec_reserve_or_raise(
                    self.py,
                    self.next,
                    required,
                    "list slice allocation failed",
                )
            }
            || self.detached.try_reserve(removed_len).is_err()
        {
            let _ = raise_exception::<u64>(self.py, "MemoryError", "list slice allocation failed");
            return false;
        }
        for &item in items {
            inc_ref_bits(self.py, item);
        }
        removed.extend(self.next_mut().splice(low..high, items.iter().copied()));
        let removed_edges = crate::object::refcount_opt::slice_heap_ref_count(&removed);
        let added_edges = crate::object::refcount_opt::slice_heap_ref_count(items);
        unsafe {
            crate::object::backing::tracked_vec_adjust_heap_edge_count(
                self.next,
                removed_edges,
                added_edges,
            )
        };
        self.detached.extend(removed);
        true
    }

    /// Publish the new runtime storage and its already-staged ABI projection,
    /// then release old runtime and transaction-only edges.
    pub(crate) unsafe fn commit(self) -> bool {
        unsafe { self.commit_with_projection(None) }
    }

    pub(crate) unsafe fn commit_with_pyobjs(
        self,
        pointers: &[*mut molt_cpython_abi::abi_types::PyObject],
    ) -> bool {
        unsafe { self.commit_with_projection(Some(pointers)) }
    }

    unsafe fn commit_with_projection(
        mut self,
        exact_pointers: Option<&[*mut molt_cpython_abi::abi_types::PyObject]>,
    ) -> bool {
        let prepared = if let Some(pointers) = exact_pointers {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.prepare_list_projection_from_pyobjs(
                self.bits,
                self.next(),
                pointers,
            )
        } else {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.prepare_list_projection(self.bits, self.next())
        };
        let Some(prepared) = prepared else {
            return false;
        };
        let mutation_guard =
            unsafe { crate::object::backing::tracked_vec_mutation_lock(self.base) };
        let live = unsafe { seq_vec_ptr(self.ptr) };
        let live_epoch = unsafe { crate::object::backing::tracked_vec_mutation_epoch(live) };
        if live != self.base || live_epoch != self.base_epoch {
            drop(mutation_guard);
            let _ = raise_exception::<u64>(
                self.py,
                "RuntimeError",
                "list mutated during mutation transaction",
            );
            return false;
        }
        unsafe { crate::object::backing::tracked_vec_swap_contents(live, self.next) };
        unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(live) };

        let contains_refs =
            unsafe { crate::object::backing::tracked_vec_heap_edge_count(live) } != 0;
        let header = unsafe { header_from_obj_ptr(self.ptr) };
        let retired_projection = unsafe {
            if contains_refs {
                (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
            } else {
                (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
            }
            prepared.publish()
        };
        drop(mutation_guard);
        drop(retired_projection);

        let old = unsafe { crate::object::backing::tracked_vec_box_from_raw(self.next) };
        self.next = std::ptr::null_mut();
        for &item in old.iter() {
            dec_ref_bits(self.py, item);
        }
        drop(old);
        for item in self.detached.drain(..) {
            dec_ref_bits(self.py, item);
        }
        self.committed = true;
        true
    }
}

/// CPython-compatible list-sort custody. The live list is published empty
/// before key/comparison callbacks, while its original runtime storage and ABI
/// projection remain detached and exclusively owned here. Completion restores
/// a permutation of the original storage without allocation, reports whether
/// callbacks mutated the live empty list, and only then releases callback-added
/// edges.
pub(crate) struct ListSortTxn<'a, 'py> {
    py: &'a PyToken<'py>,
    ptr: *mut u8,
    base: *mut Vec<u64>,
    detached: *mut Vec<u64>,
    prepared: Option<molt_cpython_abi::bridge::PreparedListProjection>,
    sort_epoch: u64,
    finished: bool,
}

impl<'a, 'py> ListSortTxn<'a, 'py> {
    /// # Safety
    /// `ptr` must be a live generic list and the caller must hold the runtime
    /// GIL. Any direct C writes must already have crossed observed ingress.
    pub(crate) unsafe fn begin(py: &'a PyToken<'py>, ptr: *mut u8) -> Option<Self> {
        crate::gil_assert();
        if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return None;
        }
        let Some(detached) = crate::object::backing::tracked_vec_box_with_capacity::<u64>(0) else {
            let _ = raise_exception::<u64>(py, "MemoryError", "list sort allocation failed");
            return None;
        };
        let base = unsafe { seq_vec_ptr(ptr) };
        let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
        if unsafe { seq_vec_ptr(ptr) } != base {
            drop(mutation_guard);
            unsafe { drop(crate::object::backing::tracked_vec_box_from_raw(detached)) };
            let _ = raise_exception::<u64>(py, "RuntimeError", "list storage changed before sort");
            return None;
        }
        let has_view =
            unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0;
        let prepared = if has_view {
            let bits = MoltObject::from_ptr(ptr).bits();
            match molt_cpython_abi::bridge::GLOBAL_BRIDGE.detach_list_projection_for_sort(bits) {
                Some(prepared) => Some(prepared),
                None => {
                    drop(mutation_guard);
                    unsafe { drop(crate::object::backing::tracked_vec_box_from_raw(detached)) };
                    return None;
                }
            }
        } else {
            None
        };
        unsafe { crate::object::backing::tracked_vec_swap_contents(base, detached) };
        let sort_epoch = unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(base) };
        unsafe {
            (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
        drop(mutation_guard);
        Some(Self {
            py,
            ptr,
            base,
            detached,
            prepared,
            sort_epoch,
            finished: false,
        })
    }

    pub(crate) fn values(&self) -> &[u64] {
        unsafe { &*self.detached }
    }

    /// Restore the sorted permutation and return whether callbacks mutated the
    /// temporarily empty live list.
    ///
    /// # Safety
    /// `ordered_values` must be a permutation of `values()`, and
    /// `projection_order` must map each destination slot to the corresponding
    /// original source slot exactly once.
    pub(crate) unsafe fn finish<I>(
        mut self,
        ordered_values: &[u64],
        projection_order: I,
    ) -> Option<bool>
    where
        I: IntoIterator<Item = usize>,
    {
        if ordered_values.len() != unsafe { (&*self.detached).len() } {
            eprintln!("molt fatal: list sort lost or duplicated an input value");
            std::process::abort();
        }
        unsafe { (&mut *self.detached).copy_from_slice(ordered_values) };
        if let Some(prepared) = self.prepared.as_mut()
            && !prepared.reorder(projection_order)
        {
            eprintln!("molt fatal: list sort projection permutation is invalid");
            std::process::abort();
        }

        let mutation_guard =
            unsafe { crate::object::backing::tracked_vec_mutation_lock(self.base) };
        let live = unsafe { seq_vec_ptr(self.ptr) };
        if live != self.base {
            eprintln!("molt fatal: list storage identity changed during sort");
            std::process::abort();
        }
        let mutated =
            unsafe { crate::object::backing::tracked_vec_mutation_epoch(live) } != self.sort_epoch;
        unsafe { crate::object::backing::tracked_vec_swap_contents(live, self.detached) };
        unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(live) };
        let contains_refs =
            unsafe { crate::object::backing::tracked_vec_heap_edge_count(live) } != 0;
        let header = unsafe { header_from_obj_ptr(self.ptr) };
        unsafe {
            if contains_refs {
                (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
            } else {
                (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
            }
        }
        let retired_projection = self
            .prepared
            .take()
            .map(|prepared| unsafe { prepared.publish() });
        drop(mutation_guard);
        drop(retired_projection);

        let callback_values =
            unsafe { crate::object::backing::tracked_vec_box_from_raw(self.detached) };
        self.detached = std::ptr::null_mut();
        for &item in callback_values.iter() {
            dec_ref_bits(self.py, item);
        }
        drop(callback_values);
        self.finished = true;
        Some(mutated)
    }
}

impl Drop for ListSortTxn<'_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            eprintln!("molt fatal: list sort transaction abandoned before restoring the list");
            std::process::abort();
        }
    }
}

#[inline]
pub(crate) unsafe fn note_in_place_mutation(ptr: *mut u8) -> u64 {
    unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(seq_vec_ptr(ptr)) }
}

/// Insert one borrowed value through the sole generic-list mutation authority.
/// Runtime and physical capacities plus the projection edge are prepared before
/// logical mutation; publication is allocation-free and O(1) amortized for
/// append.
pub(crate) unsafe fn insert(py: &PyToken<'_>, ptr: *mut u8, index: usize, item: u64) -> bool {
    unsafe { insert_with_projection(py, ptr, index, item, std::ptr::null_mut()) }
}

/// Insert while preserving an exact originating C object when the mutation
/// crossed from CPython. A NULL origin retains the runtime-only materialization
/// path.
pub(crate) unsafe fn insert_with_projection(
    py: &PyToken<'_>,
    ptr: *mut u8,
    index: usize,
    item: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let base_epoch = unsafe { crate::object::backing::tracked_vec_mutation_epoch(base) };
    let len = unsafe { (&*base).len() };
    if index > len {
        return false;
    }
    let has_view =
        unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0;
    let prepared = if has_view {
        let bits = MoltObject::from_ptr(ptr).bits();
        let Some(prepared) = (if item_ptr.is_null() {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.prepare_list_insert(bits, item)
        } else {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .prepare_list_insert_from_pyobj(bits, item, item_ptr)
        }) else {
            return false;
        };
        Some(prepared)
    } else {
        None
    };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    if unsafe { seq_vec_ptr(ptr) } != base
        || unsafe { crate::object::backing::tracked_vec_mutation_epoch(base) } != base_epoch
        || unsafe { (&*base).len() } != len
    {
        drop(mutation_guard);
        let _ = raise_exception::<u64>(py, "RuntimeError", "list mutated during insertion");
        return false;
    }
    let Some(required) = len.checked_add(1) else {
        drop(mutation_guard);
        let _ = raise_exception::<u64>(py, "MemoryError", "list is too large");
        return false;
    };
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            py,
            base,
            required,
            "list allocation failed",
        )
    } {
        drop(mutation_guard);
        return false;
    }
    inc_ref_bits(py, item);
    unsafe { (&mut *base).insert(index, item) };
    if let Some(prepared) = prepared
        && !unsafe { prepared.publish_insert(index) }
    {
        let removed = unsafe { (&mut *base).remove(index) };
        debug_assert_eq!(removed, item);
        drop(mutation_guard);
        dec_ref_bits(py, item);
        let _ = raise_exception::<u64>(
            py,
            "RuntimeError",
            "list projection changed during insertion",
        );
        return false;
    }
    let item_is_heap = crate::object::refcount_opt::is_heap_ref(item);
    unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            base,
            0,
            usize::from(item_is_heap),
        )
    };
    if item_is_heap {
        unsafe {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        }
    }
    unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(base) };
    drop(mutation_guard);
    true
}

#[inline]
pub(crate) unsafe fn append(py: &PyToken<'_>, ptr: *mut u8, item: u64) -> bool {
    unsafe { append_with_projection(py, ptr, item, std::ptr::null_mut()) }
}

pub(crate) unsafe fn append_with_projection(
    py: &PyToken<'_>,
    ptr: *mut u8,
    item: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> bool {
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let len = unsafe { list_len(ptr) };
    unsafe { insert_with_projection(py, ptr, len, item, item_ptr) }
}

/// Consume one already-owned value into a construction-only list whose caller
/// preallocated an exact upper-bound capacity. This is the sole no-INCREF
/// builder path; published lists are rejected and capacity drift aborts rather
/// than escaping resource accounting through `Vec::push` growth.
pub(crate) unsafe fn append_owned_unpublished(ptr: *mut u8, item: u64) {
    crate::gil_assert();
    if ptr.is_null()
        || unsafe { object_type_id(ptr) } != TYPE_ID_LIST
        || unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0
    {
        eprintln!("molt fatal: owned list-builder append targeted a published or invalid list");
        std::process::abort();
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    let values = unsafe { &mut *base };
    if values.len() == values.capacity() {
        eprintln!("molt fatal: owned list-builder exceeded its preallocated capacity");
        std::process::abort();
    }
    values.push(item);
    let item_is_heap = crate::object::refcount_opt::is_heap_ref(item);
    unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            base,
            0,
            usize::from(item_is_heap),
        );
        if item_is_heap {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        }
        crate::object::backing::tracked_vec_bump_mutation_epoch(base);
    }
    drop(mutation_guard);
}

/// Replace one runtime slot and transfer the displaced owned edge to the
/// caller. This is the common primitive for an unpublished runtime-only store
/// and the runtime half of a staged CPython projection store. When a view is
/// published, the bridge has already staged the physical ownership edge and
/// publishes it immediately after this returns; when no view exists, the
/// caller simply releases the displaced edge after observing the result.
pub(crate) unsafe fn replace_one_transferring_displaced(
    py: &PyToken<'_>,
    ptr: *mut u8,
    index: usize,
    item: u64,
) -> Option<u64> {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return None;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    if index >= unsafe { (&*base).len() } {
        return None;
    }
    inc_ref_bits(py, item);
    let old = unsafe { std::mem::replace(&mut (&mut *base)[index], item) };
    let contains_refs = unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            base,
            usize::from(crate::object::refcount_opt::is_heap_ref(old)),
            usize::from(crate::object::refcount_opt::is_heap_ref(item)),
        )
    } != 0;
    unsafe {
        if contains_refs {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        } else {
            (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
        crate::object::backing::tracked_vec_bump_mutation_epoch(base);
    }
    drop(mutation_guard);
    Some(old)
}

/// Replace one item with a pre-staged delta projection. The displaced runtime
/// and C edges are released only after the new generation is published.
pub(crate) unsafe fn replace_one(py: &PyToken<'_>, ptr: *mut u8, index: usize, item: u64) -> bool {
    unsafe { replace_one_with_projection(py, ptr, index, item, std::ptr::null_mut()) }
}

pub(crate) unsafe fn replace_one_with_projection(
    py: &PyToken<'_>,
    ptr: *mut u8,
    index: usize,
    item: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let base_epoch = unsafe { crate::object::backing::tracked_vec_mutation_epoch(base) };
    let len = unsafe { (&*base).len() };
    if index >= len {
        return false;
    }
    let has_view =
        unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0;
    let prepared = if has_view {
        let bits = MoltObject::from_ptr(ptr).bits();
        let prepared = if item_ptr.is_null() {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.prepare_list_set(bits, item)
        } else {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .prepare_list_set_from_pyobj(bits, item, item_ptr)
        };
        let Some(prepared) = prepared else {
            return false;
        };
        Some(prepared)
    } else {
        None
    };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    if unsafe { seq_vec_ptr(ptr) } != base
        || unsafe { crate::object::backing::tracked_vec_mutation_epoch(base) } != base_epoch
        || unsafe { (&*base).len() } != len
    {
        drop(mutation_guard);
        let _ = raise_exception::<u64>(py, "RuntimeError", "list mutated during indexed store");
        return false;
    }
    inc_ref_bits(py, item);
    let old = unsafe { std::mem::replace(&mut (&mut *base)[index], item) };
    let retired = if let Some(prepared) = prepared {
        let Some(retired) = (unsafe { prepared.publish_set(index) }) else {
            unsafe { (&mut *base)[index] = old };
            drop(mutation_guard);
            dec_ref_bits(py, item);
            let _ = raise_exception::<u64>(
                py,
                "RuntimeError",
                "list projection changed during indexed store",
            );
            return false;
        };
        Some(retired)
    } else {
        None
    };
    let contains_refs = unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            base,
            usize::from(crate::object::refcount_opt::is_heap_ref(old)),
            usize::from(crate::object::refcount_opt::is_heap_ref(item)),
        )
    } != 0;
    unsafe {
        if contains_refs {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        } else {
            (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
        crate::object::backing::tracked_vec_bump_mutation_epoch(base);
    }
    drop(mutation_guard);
    drop(retired);
    dec_ref_bits(py, old);
    true
}

/// Remove one item and return a new owned runtime reference to it.
pub(crate) unsafe fn pop(py: &PyToken<'_>, ptr: *mut u8, index: usize) -> Option<u64> {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return None;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    if index >= unsafe { (&*base).len() } {
        return None;
    }
    let value = unsafe { (&*base)[index] };
    inc_ref_bits(py, value);
    let removed = unsafe { (&mut *base).remove(index) };
    debug_assert_eq!(removed, value);
    let has_view =
        unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0;
    let retired = if has_view {
        let Some(retired) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .publish_list_remove(MoltObject::from_ptr(ptr).bits(), index)
        else {
            unsafe { (&mut *base).insert(index, value) };
            drop(mutation_guard);
            dec_ref_bits(py, value);
            let _ =
                raise_exception::<u64>(py, "RuntimeError", "list projection changed during pop");
            return None;
        };
        Some(retired)
    } else {
        None
    };
    let contains_refs = unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            base,
            usize::from(crate::object::refcount_opt::is_heap_ref(value)),
            0,
        )
    } != 0;
    if !contains_refs {
        unsafe {
            (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
    }
    unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(base) };
    drop(mutation_guard);
    drop(retired);
    dec_ref_bits(py, removed);
    Some(value)
}

/// Clear logical length and detach the charged runtime buffer without a
/// replacement allocation. Both authorities publish empty before any edge is
/// released.
pub(crate) unsafe fn clear(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    let displaced = unsafe { crate::object::backing::tracked_vec_take_contents(base) };
    let has_view =
        unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0;
    let retired = if has_view {
        let Some(retired) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .publish_list_clear(MoltObject::from_ptr(ptr).bits())
        else {
            eprintln!("molt fatal: clean list projection changed during clear");
            std::process::abort();
        };
        Some(retired)
    } else {
        None
    };
    unsafe {
        (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        crate::object::backing::tracked_vec_bump_mutation_epoch(base);
    }
    drop(mutation_guard);
    drop(retired);
    for &item in displaced.iter() {
        dec_ref_bits(py, item);
    }
    drop(displaced);
    true
}

/// Reverse both runtime and physical views in place without allocation or
/// reference-count churn.
pub(crate) unsafe fn reverse(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    unsafe { (&mut *base).reverse() };
    if unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0
        && !molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .publish_list_reverse(MoltObject::from_ptr(ptr).bits())
    {
        unsafe { (&mut *base).reverse() };
        drop(mutation_guard);
        let _ = raise_exception::<u64>(
            _py,
            "RuntimeError",
            "list projection changed during reverse",
        );
        return false;
    }
    unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(base) };
    drop(mutation_guard);
    true
}

/// Replace one contiguous range through the sole published-list mutation
/// authority. Allocation and replacement ownership are prepared before the
/// live vector changes. The new canonical shape and generation are published
/// before any removed edge is released, so reentrant finalizers observe the
/// completed mutation.
///
/// # Safety
/// `ptr` must be a live generic list and the caller must hold the runtime GIL.
pub(crate) unsafe fn replace_range(
    py: &PyToken<'_>,
    ptr: *mut u8,
    low: usize,
    high: usize,
    items: &[u64],
) -> bool {
    unsafe { replace_range_with_projection(py, ptr, low, high, items, None) }
}

pub(crate) unsafe fn replace_range_with_projection(
    py: &PyToken<'_>,
    ptr: *mut u8,
    low: usize,
    high: usize,
    items: &[u64],
    future_pointers: Option<&[*mut molt_cpython_abi::abi_types::PyObject]>,
) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let len = unsafe { list_len(ptr) };
    if low > high || high > len {
        return false;
    }
    let removed_len = high - low;
    let Some(result_len) = len
        .checked_sub(removed_len)
        .and_then(|base| base.checked_add(items.len()))
    else {
        let _ = raise_exception::<u64>(py, "MemoryError", "list slice allocation failed");
        return false;
    };
    if future_pointers.is_some_and(|pointers| pointers.len() != result_len) {
        let _ = raise_exception::<u64>(py, "SystemError", "list slice projection length mismatch");
        return false;
    }
    if removed_len == 0 && items.is_empty() {
        return true;
    }
    if removed_len == 0 && items.len() == 1 {
        let item_ptr = future_pointers
            .map(|pointers| pointers[low])
            .unwrap_or(std::ptr::null_mut());
        return unsafe { insert_with_projection(py, ptr, low, items[0], item_ptr) };
    }
    if removed_len == 1 && items.len() == 1 {
        let item_ptr = future_pointers
            .map(|pointers| pointers[low])
            .unwrap_or(std::ptr::null_mut());
        return unsafe { replace_one_with_projection(py, ptr, low, items[0], item_ptr) };
    }
    if removed_len == 1 && items.is_empty() {
        let Some(removed) = (unsafe { pop(py, ptr, low) }) else {
            return false;
        };
        dec_ref_bits(py, removed);
        return true;
    }
    if low == 0 && high == len && items.is_empty() {
        return unsafe { clear(py, ptr) };
    }
    if unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0 {
        let Some(mut txn) = (unsafe { ListMutationTxn::begin(py, ptr) }) else {
            return false;
        };
        if !txn.replace_range(low, high, items) {
            return false;
        }
        return if let Some(pointers) = future_pointers {
            unsafe { txn.commit_with_pyobjs(pointers) }
        } else {
            unsafe { txn.commit() }
        };
    }

    let required = result_len;
    let mut removed = Vec::new();
    if removed.try_reserve_exact(removed_len).is_err() {
        let _ = raise_exception::<u64>(py, "MemoryError", "list slice allocation failed");
        return false;
    }
    let vec_ptr = unsafe { seq_vec_ptr(ptr) };
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            py,
            vec_ptr,
            required,
            "list slice allocation failed",
        )
    } {
        return false;
    }
    for &item in items {
        inc_ref_bits(py, item);
    }
    let vec = unsafe { &mut *vec_ptr };
    removed.extend(vec.splice(low..high, items.iter().copied()));
    let contains_refs = unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            vec_ptr,
            crate::object::refcount_opt::slice_heap_ref_count(&removed),
            crate::object::refcount_opt::slice_heap_ref_count(items),
        )
    } != 0;
    let header = unsafe { header_from_obj_ptr(ptr) };
    unsafe {
        if contains_refs {
            (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        } else {
            (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
        note_in_place_mutation(ptr);
    }
    for item in removed {
        dec_ref_bits(py, item);
    }
    true
}

/// Append an already-materialized slice in one allocation-checked publication.
/// Input values are borrowed; the list acquires its own runtime ownership.
pub(crate) unsafe fn extend_from_slice(py: &PyToken<'_>, ptr: *mut u8, items: &[u64]) -> bool {
    if items.is_empty() {
        return true;
    }
    if items.len() == 1 {
        return unsafe { append(py, ptr, items[0]) };
    }
    let len = unsafe { list_len(ptr) };
    unsafe { replace_range(py, ptr, len, len, items) }
}

/// Replace a fixed set of distinct indices in one publication. This is the
/// extended-slice assignment authority.
pub(crate) unsafe fn replace_indices(
    py: &PyToken<'_>,
    ptr: *mut u8,
    indices: &[usize],
    items: &[u64],
) -> bool {
    crate::gil_assert();
    if ptr.is_null()
        || unsafe { object_type_id(ptr) } != TYPE_ID_LIST
        || indices.len() != items.len()
    {
        return false;
    }
    let len = unsafe { list_len(ptr) };
    if indices.iter().any(|&index| index >= len) {
        return false;
    }
    if indices.is_empty() {
        return true;
    }
    if indices.len() == 1 {
        return unsafe { replace_one(py, ptr, indices[0], items[0]) };
    }
    if unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0 {
        let Some(mut txn) = (unsafe { ListMutationTxn::begin(py, ptr) }) else {
            return false;
        };
        return txn.set_indices(indices, items) && unsafe { txn.commit() };
    }

    let mut removed = Vec::new();
    if removed.try_reserve_exact(indices.len()).is_err() {
        let _ = raise_exception::<u64>(py, "MemoryError", "list allocation failed");
        return false;
    }
    for &item in items {
        inc_ref_bits(py, item);
    }
    let vec_ptr = unsafe { seq_vec_ptr(ptr) };
    let values = unsafe { &mut *vec_ptr };
    for (&index, &item) in indices.iter().zip(items) {
        removed.push(std::mem::replace(&mut values[index], item));
    }
    let contains_refs = unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            vec_ptr,
            crate::object::refcount_opt::slice_heap_ref_count(&removed),
            crate::object::refcount_opt::slice_heap_ref_count(items),
        )
    } != 0;
    let header = unsafe { header_from_obj_ptr(ptr) };
    unsafe {
        if contains_refs {
            (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        } else {
            (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
        note_in_place_mutation(ptr);
    }
    for item in removed {
        dec_ref_bits(py, item);
    }
    true
}

/// Remove indices supplied in an order that stays valid as the vector shrinks
/// (descending original index for positive slices; natural order for negative
/// slices). Publication precedes every removed-edge release.
pub(crate) unsafe fn remove_indices(
    py: &PyToken<'_>,
    ptr: *mut u8,
    removal_order: &[usize],
) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let mut remaining = unsafe { list_len(ptr) };
    for &index in removal_order {
        if index >= remaining {
            return false;
        }
        remaining -= 1;
    }
    if removal_order.is_empty() {
        return true;
    }
    if removal_order.len() == 1 {
        let Some(removed) = (unsafe { pop(py, ptr, removal_order[0]) }) else {
            return false;
        };
        dec_ref_bits(py, removed);
        return true;
    }
    if unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0 {
        let Some(mut txn) = (unsafe { ListMutationTxn::begin(py, ptr) }) else {
            return false;
        };
        return txn.remove_indices(removal_order) && unsafe { txn.commit() };
    }

    let mut removed = Vec::new();
    if removed.try_reserve_exact(removal_order.len()).is_err() {
        let _ = raise_exception::<u64>(py, "MemoryError", "list allocation failed");
        return false;
    }
    let vec_ptr = unsafe { seq_vec_ptr(ptr) };
    let values = unsafe { &mut *vec_ptr };
    for &index in removal_order {
        removed.push(values.remove(index));
    }
    let contains_refs = unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            vec_ptr,
            crate::object::refcount_opt::slice_heap_ref_count(&removed),
            0,
        )
    } != 0;
    let header = unsafe { header_from_obj_ptr(ptr) };
    unsafe {
        if contains_refs {
            (*header).fetch_or_flags(HEADER_FLAG_CONTAINS_REFS);
        } else {
            (*header).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        }
        note_in_place_mutation(ptr);
    }
    for item in removed {
        dec_ref_bits(py, item);
    }
    true
}

pub(crate) unsafe fn repeat(py: &PyToken<'_>, ptr: *mut u8, count: usize) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let len = unsafe { list_len(ptr) };
    if count == 0 {
        return unsafe { replace_range(py, ptr, 0, len, &[]) };
    }
    if count == 1 || len == 0 {
        return true;
    }
    let Some(total) = len.checked_mul(count) else {
        let _ = raise_exception::<u64>(
            py,
            "OverflowError",
            "cannot fit 'int' into an index-sized integer",
        );
        return false;
    };
    if unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0 {
        let Some(mut txn) = (unsafe { ListMutationTxn::begin(py, ptr) }) else {
            return false;
        };
        return txn.repeat(count) && unsafe { txn.commit() };
    }
    let vec_ptr = unsafe { seq_vec_ptr(ptr) };
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            py,
            vec_ptr,
            total,
            "list allocation failed",
        )
    } {
        return false;
    }
    let values = unsafe { &mut *vec_ptr };
    let initial_edges = unsafe { crate::object::backing::tracked_vec_heap_edge_count(vec_ptr) };
    for _ in 1..count {
        for index in 0..len {
            let item = values[index];
            inc_ref_bits(py, item);
            values.push(item);
        }
    }
    unsafe {
        crate::object::backing::tracked_vec_adjust_heap_edge_count(
            vec_ptr,
            0,
            initial_edges.saturating_mul(count - 1),
        )
    };
    unsafe { note_in_place_mutation(ptr) };
    true
}

/// Swap two published list slots without ownership or allocation churn.
pub(crate) unsafe fn swap_indices(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    left: usize,
    right: usize,
) -> bool {
    crate::gil_assert();
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return false;
    }
    let base = unsafe { seq_vec_ptr(ptr) };
    let mutation_guard = unsafe { crate::object::backing::tracked_vec_mutation_lock(base) };
    let values = unsafe { &mut *base };
    if left >= values.len() || right >= values.len() {
        return false;
    }
    if left == right {
        return true;
    }
    values.swap(left, right);
    if unsafe { (*header_from_obj_ptr(ptr)).load_flags() } & HEADER_FLAG_HAS_ABI_VIEW != 0
        && !molt_cpython_abi::bridge::GLOBAL_BRIDGE.publish_list_swap(
            MoltObject::from_ptr(ptr).bits(),
            left,
            right,
        )
    {
        values.swap(left, right);
        return false;
    }
    unsafe { crate::object::backing::tracked_vec_bump_mutation_epoch(base) };
    drop(mutation_guard);
    true
}

impl Drop for ListMutationTxn<'_, '_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if !self.next.is_null() {
            let next = unsafe { crate::object::backing::tracked_vec_box_from_raw(self.next) };
            for &item in next.iter() {
                dec_ref_bits(self.py, item);
            }
            drop(next);
            self.next = std::ptr::null_mut();
        }
        for item in self.detached.drain(..) {
            dec_ref_bits(self.py, item);
        }
    }
}
