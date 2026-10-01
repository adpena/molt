//! Physical Unicode, tuple and list projection preparation and transactions.
//!
//! Storage remains in the parent bridge; this module preserves the single
//! projection-ledger authority and the existing shard-lock/release ordering.

use super::*;

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

    pub(super) fn publish_prepared_list_projection(
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
    pub(super) fn commit_list_view_inner(&self, bits: AbiHandle, require_complete: bool) -> bool {
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
