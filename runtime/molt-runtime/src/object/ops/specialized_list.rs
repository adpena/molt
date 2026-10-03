use super::*;

/// A raw storage view must not span Python's __index__ callbacks. Promote
/// before such a callback, including slice components; inert keys keep raw IO.
unsafe fn specialized_key_requires_boxed_list(
    py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    expected: u32,
) -> bool {
    unsafe {
        if object_type_id(ptr) != expected {
            return true;
        }
        let inert = |bits| {
            let value = obj_from_bits(bits);
            value.is_none()
                || index_i64_integral_bits(bits).is_some()
                || bigint_ptr_from_bits(bits).is_some()
                || int_subclass_value_bits_raw(bits).is_some()
        };
        let key = obj_from_bits(key_bits);
        let direct = (!key.is_none() && inert(key_bits))
            || key.as_ptr().is_some_and(|slice| {
                object_type_id(slice) == TYPE_ID_SLICE
                    && inert(slice_start_bits(slice))
                    && inert(slice_stop_bits(slice))
                    && inert(slice_step_bits(slice))
            });
        if !direct {
            crate::object::ops_list::promote_specialized_list_to_list(py, ptr);
        }
        !direct
    }
}

/// Physical ABI entrypoints require a live flat-storage object. A semantic
/// list[int] annotation never permits interpreting an ordinary Vec as storage.
fn flat_list_int_ptr(list_bits: u64) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(list_bits).as_ptr()
        && unsafe { object_type_id(ptr) == TYPE_ID_LIST_INT }
    {
        return Some(ptr);
    }
    crate::with_gil_entry_nopanic!(py, {
        raise_exception::<Option<*mut u8>>(py, "SystemError", "flat list storage contract violated")
    })
}

fn list_specialized_index_from_bits(index_bits: u64) -> Option<i64> {
    if let Some(i) = index_i64_integral_bits(index_bits) {
        return Some(i);
    }
    crate::with_gil_entry_nopanic!(_py, { sequence_index_i64(_py, index_bits, "list") })
}

#[inline]
fn list_index_out_of_range_error() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(_py, "IndexError", "list index out of range")
    })
}

#[inline]
fn list_assignment_out_of_range_error() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(_py, "IndexError", "list assignment index out of range")
    })
}

unsafe fn list_int_slice_to_flat_list(_py: &PyToken<'_>, ptr: *mut u8, slice_ptr: *mut u8) -> u64 {
    unsafe {
        let len = list_len(ptr) as isize;
        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
        let (start, stop, step) =
            match normalize_slice_indices(_py, len, start_obj, stop_obj, step_obj) {
                Ok(vals) => vals,
                Err(err) => return slice_error(_py, err),
            };
        let elems = crate::object::layout::list_int_vec_ref(ptr);
        match alloc_list_int_from_normalized_slice(_py, elems.as_slice(), start, stop, step) {
            Ok(out_ptr) => MoltObject::from_ptr(out_ptr).bits(),
            Err(bits) => bits,
        }
    }
}

unsafe fn list_bool_slice_to_flat_list(_py: &PyToken<'_>, ptr: *mut u8, slice_ptr: *mut u8) -> u64 {
    unsafe {
        let len = list_len(ptr) as isize;
        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
        let (start, stop, step) =
            match normalize_slice_indices(_py, len, start_obj, stop_obj, step_obj) {
                Ok(vals) => vals,
                Err(err) => return slice_error(_py, err),
            };
        let elems = crate::object::layout::list_bool_vec_ref(ptr);
        match alloc_list_bool_from_normalized_slice(_py, elems.as_slice(), start, stop, step) {
            Ok(out_ptr) => MoltObject::from_ptr(out_ptr).bits(),
            Err(bits) => bits,
        }
    }
}

#[inline]
fn normalized_slice_len(start: isize, stop: isize, step: isize) -> usize {
    debug_assert_ne!(step, 0);
    let mut len = 0usize;
    let mut idx = start;
    if step > 0 {
        while idx < stop {
            len = len.saturating_add(1);
            let Some(next) = idx.checked_add(step) else {
                break;
            };
            idx = next;
        }
    } else {
        while idx > stop {
            len = len.saturating_add(1);
            let Some(next) = idx.checked_add(step) else {
                break;
            };
            idx = next;
        }
    }
    len
}

unsafe fn alloc_list_int_from_normalized_slice(
    _py: &PyToken<'_>,
    elems: &[i64],
    start: isize,
    stop: isize,
    step: isize,
) -> Result<*mut u8, u64> {
    if step == 1 {
        let s = start as usize;
        let e = (stop as usize).max(s);
        return crate::object::builders::alloc_list_int_from_raw_slice(_py, &elems[s..e]);
    }

    let len = normalized_slice_len(start, stop, step);
    let mut idx = start;
    crate::object::builders::alloc_list_int_from_raw_iter(_py, len, |_| {
        let current = idx;
        let Some(next) = idx.checked_add(step) else {
            return elems[current as usize];
        };
        idx = next;
        elems[current as usize]
    })
}

unsafe fn alloc_list_bool_from_normalized_slice(
    _py: &PyToken<'_>,
    elems: &[u8],
    start: isize,
    stop: isize,
    step: isize,
) -> Result<*mut u8, u64> {
    if step == 1 {
        let s = start as usize;
        let e = (stop as usize).max(s);
        return crate::object::builders::alloc_list_bool_from_raw_slice(_py, &elems[s..e]);
    }

    let len = normalized_slice_len(start, stop, step);
    let mut idx = start;
    crate::object::builders::alloc_list_bool_from_raw_iter(_py, len, |_| {
        let current = idx;
        let Some(next) = idx.checked_add(step) else {
            return elems[current as usize];
        };
        idx = next;
        elems[current as usize]
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem(list_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(ptr) = obj_from_bits(list_bits).as_ptr() else {
            return molt_index(list_bits, index_bits);
        };
        unsafe {
            if specialized_key_requires_boxed_list(py, ptr, index_bits, TYPE_ID_LIST_INT) {
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return molt_index(list_bits, index_bits);
            }
            if let Some(slice) = obj_from_bits(index_bits).as_ptr()
                && object_type_id(slice) == TYPE_ID_SLICE
            {
                return list_int_slice_to_flat_list(py, ptr, slice);
            }
            let Some(mut index) = list_specialized_index_from_bits(index_bits) else {
                return MoltObject::none().bits();
            };
            let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
            let len = storage.len as i64;
            if index < 0 {
                index += len;
            }
            if index < 0 || index >= len {
                return list_index_out_of_range_error();
            }
            let raw = *storage.data.add(index as usize);
            MoltObject::from_int(raw).bits()
        }
    })
}

/// Raw-register fast path for list[int] getitem.
/// Takes a raw i64 index (NOT NaN-boxed) and returns a raw i64 value (NOT NaN-boxed).
/// Eliminates NaN-box/unbox round-trips when both index and result stay in raw_int_shadow.
/// Uses the same bounds and physical-layout contract as the checked ABI.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_raw(list_bits: u64, raw_index: i64) -> i64 {
    molt_list_int_getitem_raw_checked(list_bits, raw_index)
}

/// Raw-register list[int] getitem with Python exception semantics.
///
/// Takes a raw i64 index and returns the raw i64 element. On out-of-bounds it
/// raises the same IndexError as the boxed getitem path and returns 0 only as
/// an exception-continuation sentinel.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_raw_checked(list_bits: u64, raw_index: i64) -> i64 {
    let Some(ptr) = flat_list_int_ptr(list_bits) else {
        return 0;
    };
    unsafe {
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        let index = if raw_index < 0 {
            raw_index + len
        } else {
            raw_index
        };
        if index < 0 || index >= len {
            let _ = list_index_out_of_range_error();
            return 0;
        }
        *storage.data.add(index as usize)
    }
}

/// Set element in a specialized list[int].
/// Expects a NaN-boxed int value — extracts raw i64 and stores directly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(ptr) = obj_from_bits(list_bits).as_ptr() else {
            return molt_store_index(list_bits, index_bits, value_bits);
        };
        unsafe {
            if specialized_key_requires_boxed_list(py, ptr, index_bits, TYPE_ID_LIST_INT) {
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(list_bits, index_bits, value_bits);
            }
            let slice = obj_from_bits(index_bits)
                .as_ptr()
                .is_some_and(|key| object_type_id(key) == TYPE_ID_SLICE);
            let admitted = crate::object::layout::InlineListInt::from_bits(value_bits);
            if slice || admitted.is_none() {
                crate::object::ops_list::promote_specialized_list_to_list(py, ptr);
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(list_bits, index_bits, value_bits);
            }
            let value = admitted.unwrap();
            let Some(mut index) = list_specialized_index_from_bits(index_bits) else {
                return MoltObject::none().bits();
            };
            let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
            let len = storage.len as i64;
            if index < 0 {
                index += len;
            }
            if index < 0 || index >= len {
                return list_assignment_out_of_range_error();
            }
            *storage.data.add(index as usize) = value.raw();
            list_bits
        }
    })
}

/// Get element from a specialized list[bool].
/// Returns a NaN-boxed bool (True or False).
/// No refcounting needed -- bools are inline NaN-boxed values.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_bool_getitem(list_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(ptr) = obj_from_bits(list_bits).as_ptr() else {
            return molt_index(list_bits, index_bits);
        };
        unsafe {
            if specialized_key_requires_boxed_list(py, ptr, index_bits, TYPE_ID_LIST_BOOL) {
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return molt_index(list_bits, index_bits);
            }
            if let Some(slice) = obj_from_bits(index_bits).as_ptr()
                && object_type_id(slice) == TYPE_ID_SLICE
            {
                return list_bool_slice_to_flat_list(py, ptr, slice);
            }
            let Some(mut index) = list_specialized_index_from_bits(index_bits) else {
                return MoltObject::none().bits();
            };
            let storage = &*crate::object::layout::list_bool_storage_ptr(ptr);
            let len = storage.len as i64;
            if index < 0 {
                index += len;
            }
            if index < 0 || index >= len {
                return list_index_out_of_range_error();
            }
            let raw = *storage.data.add(index as usize);
            MoltObject::from_bool(raw != 0).bits()
        }
    })
}

/// Set element in a specialized list[bool].
/// Accepts exact bools; every other owner promotes into canonical boxed storage.
/// No refcounting needed -- bools are inline NaN-boxed values.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_bool_setitem(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(ptr) = obj_from_bits(list_bits).as_ptr() else {
            return molt_store_index(list_bits, index_bits, value_bits);
        };
        unsafe {
            if specialized_key_requires_boxed_list(py, ptr, index_bits, TYPE_ID_LIST_BOOL) {
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(list_bits, index_bits, value_bits);
            }
            let slice = obj_from_bits(index_bits)
                .as_ptr()
                .is_some_and(|key| object_type_id(key) == TYPE_ID_SLICE);
            let admitted = obj_from_bits(value_bits).as_bool();
            if slice || admitted.is_none() {
                crate::object::ops_list::promote_specialized_list_to_list(py, ptr);
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(list_bits, index_bits, value_bits);
            }
            let value = admitted.unwrap();
            let Some(mut index) = list_specialized_index_from_bits(index_bits) else {
                return MoltObject::none().bits();
            };
            let storage = &mut *crate::object::layout::list_bool_storage_ptr(ptr);
            let len = storage.len as i64;
            if index < 0 {
                index += len;
            }
            if index < 0 || index >= len {
                return list_assignment_out_of_range_error();
            }
            *storage.data.add(index as usize) = u8::from(value);
            list_bits
        }
    })
}

/// Borrow the flat inline-integer data pointer until mutation or escape.
/// The caller must hold a current physical storage proof, not a semantic type.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_data(list_bits: u64) -> u64 {
    let Some(ptr) = flat_list_int_ptr(list_bits) else {
        return 0;
    };
    unsafe { (*crate::object::layout::list_int_storage_ptr(ptr)).data as u64 }
}

/// Return the length of an admitted flat inline-integer list as a raw u64.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_len_raw(list_bits: u64) -> u64 {
    let Some(ptr) = flat_list_int_ptr(list_bits) else {
        return 0;
    };
    unsafe { (*crate::object::layout::list_int_storage_ptr(ptr)).len as u64 }
}

/// Get length of a specialized list[int].
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_len(list_bits: u64) -> u64 {
    molt_len(list_bits)
}

/// Check if value is truthy in a specialized list[int] element context.
/// Raw i64: 0 is falsy, everything else is truthy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_truthy(list_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let item = molt_list_int_getitem(list_bits, index_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, item);
            return MoltObject::none().bits();
        }
        let result = MoltObject::from_bool(is_truthy(_py, obj_from_bits(item))).bits();
        dec_ref_bits(_py, item);
        result
    })
}

/// Unchecked list getitem — used when BCE (Bounds Check Elimination) has proven
/// the index is in bounds.
///
/// # Safety
/// The caller guarantees:
///   - `list_bits` is a valid NaN-boxed heap pointer to an exact builtin list
///     with TYPE_ID_LIST storage; a List type hint does not establish this.
///   - `0 <= index < len(list)` — no bounds check is performed.
///   - The list is not mutated concurrently (GIL must be held by the caller).
///
/// Violating any of these preconditions causes undefined behaviour.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_getitem_unchecked(list_bits: u64, index: i64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    // Safety: caller guarantees list_bits is a valid list heap pointer.
    let ptr = unsafe { list_obj.as_ptr().unwrap_unchecked() };
    let mut val = 0;
    // The owned read pins the element under the backing lock and transfers
    // that reference directly to the caller.
    if crate::object::seq_access::read_item_owned(ptr, index as usize, &mut val) == 0 {
        return 0;
    }
    val
}

// ---------------------------------------------------------------------------
// CPython specialized bytecode fast paths (BINARY_SUBSCR_LIST_INT,
// STORE_SUBSCR_LIST_INT, COMPARE_OP_INT, COMPARE_OP_STR).
// These functions are extern "C" so they can be emitted as direct calls by
// the AOT compiler back-end instead of routing through the generic dispatch.
// ---------------------------------------------------------------------------

/// Fast path: integer index into a list (BINARY_SUBSCR_LIST_INT).
///
/// Handles positive and negative indexing with direct array access.
/// On any failure (wrong type tags, out-of-bounds) falls through to
/// the full `molt_index` slow path.
///
/// Returns the element bits on success, or `u64::MAX` as a sentinel to
/// signal the caller to fall back to `molt_index`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_getitem_int_fast(list_bits: u64, index_bits: u64) -> u64 {
    // 1. Fast tag check: index must be a NaN-boxed int.
    let index_obj = obj_from_bits(index_bits);
    if !index_obj.is_int() {
        return molt_index(list_bits, index_bits);
    }
    // 2. List must be a heap pointer.
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return molt_index(list_bits, index_bits);
    };
    unsafe {
        // A semantic List hint does not prove the builtin Python class.
        // Class-bearing receivers re-enter source special-method dispatch.
        if object_class_bits(ptr) != 0 {
            return molt_index(list_bits, index_bits);
        }
        // 3. Must actually be a list (regular or specialized).
        let tid = object_type_id(ptr);
        if tid == TYPE_ID_LIST_BOOL {
            // list[bool] fast path — u8 storage, no refcount needed.
            let mut idx = index_obj.as_int_unchecked();
            let storage = &*crate::object::layout::list_bool_storage_ptr(ptr);
            let len = storage.len as i64;
            if idx < 0 {
                idx += len;
            }
            if idx < 0 || idx >= len {
                return molt_index(list_bits, index_bits);
            }
            return MoltObject::from_bool(*storage.data.add(idx as usize) != 0).bits();
        }
        if tid != TYPE_ID_LIST {
            return molt_index(list_bits, index_bits);
        }
        // 4. Extract index and list length.
        let mut idx = index_obj.as_int_unchecked();
        let len = crate::object::seq_access::len(ptr) as i64;
        // 5. Handle negative indexing.
        if idx < 0 {
            idx += len;
        }
        // 6. Bounds check.
        if idx < 0 || idx >= len {
            return molt_index(list_bits, index_bits);
        }
        // 7. Lock-pinned load. `read_item_owned` uses the no-op reentrant GIL
        // lane when compiled code already owns it and transfers one reference
        // to the result stack.
        let mut val = 0;
        if crate::object::seq_access::read_item_owned(ptr, idx as usize, &mut val) == 0 {
            return molt_index(list_bits, index_bits);
        }
        val
    }
}

/// List getitem with a raw i64 index (no NaN-box tag check needed).
///
/// Called when the compiler has proven the index is an integer and holds
/// it in a raw i64 Cranelift register. Skips the is_int() tag check and
/// the as_int_unchecked() unbox — the index is already a plain i64.
///
/// The list operand is still NaN-boxed (it's a heap pointer).
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_list_getitem_raw_idx(list_bits: u64, raw_idx: i64) -> u64 {
    if let Some(ptr) = obj_from_bits(list_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_LIST && object_class_bits(ptr) == 0 {
                let len = crate::object::seq_access::len(ptr) as i64;
                let index = if raw_idx < 0 { raw_idx + len } else { raw_idx };
                if index >= 0 && index < len {
                    let mut value = 0;
                    if crate::object::seq_access::read_item_owned(ptr, index as usize, &mut value)
                        != 0
                    {
                        return value;
                    }
                }
            }
        }
    }
    crate::with_gil_entry_nopanic!(_py, {
        let index = int_bits_from_i64(_py, raw_idx);
        if exception_pending(_py) {
            dec_ref_bits(_py, index);
            return MoltObject::none().bits();
        }
        let value = molt_index(list_bits, index);
        dec_ref_bits(_py, index);
        value
    })
}

// ── Specialized list[int] operations ────────────────────────────────
//
// When the compiler proves a list contains only integers, it uses these
// specialized functions that store raw i64 values without NaN-boxing.
// Element access is a single array load + box_int on return.
// A supplied inline integer selects flat storage. Every other Python value
// remains in boxed list storage with its original object ownership.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_new(count: u64, fill_value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(count) = crate::object::ops_arith::sequence_repeat_count(_py, count) else {
            return MoltObject::none().bits();
        };
        let len = count.max(0) as usize;
        match crate::object::builders::alloc_list_int_from_fill(_py, len, fill_value) {
            Ok(ptr) => MoltObject::from_ptr(ptr).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_fill_new(count: u64, fill_value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(count) = crate::object::ops_arith::sequence_repeat_count(_py, count) else {
            return MoltObject::none().bits();
        };
        let ptr = crate::object::builders::alloc_list_filled(
            _py,
            count.max(0) as usize,
            obj_from_bits(fill_value),
        );
        if ptr.is_null() {
            if !exception_pending(_py) {
                crate::record_memory_error_without_allocation(_py);
            }
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_int_slice_preserves_flat_storage() {
        crate::with_gil_entry_nopanic!(_py, {
            let source_ptr =
                crate::object::builders::alloc_list_int_from_raw_slice(_py, &[10, 20, 30, 40, 50])
                    .expect("source list[int]");
            let source_bits = MoltObject::from_ptr(source_ptr).bits();
            let slice_bits = molt_slice_new(
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(2).bits(),
            );

            let out_bits = molt_list_int_getitem(source_bits, slice_bits);
            let out_ptr = obj_from_bits(out_bits).as_ptr().expect("slice result");
            assert_eq!(unsafe { object_type_id(out_ptr) }, TYPE_ID_LIST_INT);
            let out = unsafe { crate::object::layout::list_int_vec_ref(out_ptr) };
            assert_eq!(out.as_slice(), &[20, 40]);

            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, slice_bits);
            dec_ref_bits(_py, source_bits);
        });
    }

    #[test]
    fn list_bool_reverse_slice_preserves_flat_storage() {
        crate::with_gil_entry_nopanic!(_py, {
            let source_ptr =
                crate::object::builders::alloc_list_bool_from_raw_slice(_py, &[1, 0, 1, 1, 0])
                    .expect("source list[bool]");
            let source_bits = MoltObject::from_ptr(source_ptr).bits();
            let slice_bits = molt_slice_new(
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                MoltObject::from_int(-2).bits(),
            );

            let out_bits = molt_list_bool_getitem(source_bits, slice_bits);
            let out_ptr = obj_from_bits(out_bits).as_ptr().expect("slice result");
            assert_eq!(unsafe { object_type_id(out_ptr) }, TYPE_ID_LIST_BOOL);
            let out = unsafe { crate::object::layout::list_bool_vec_ref(out_ptr) };
            assert_eq!(out.as_slice(), &[0, 1, 1]);

            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, slice_bits);
            dec_ref_bits(_py, source_bits);
        });
    }

    #[test]
    fn list_copy_preserves_flat_int_storage_through_shared_builder() {
        crate::with_gil_entry_nopanic!(_py, {
            let source_ptr =
                crate::object::builders::alloc_list_int_from_raw_slice(_py, &[2, 3, 5, 7])
                    .expect("source list[int]");
            let source_bits = MoltObject::from_ptr(source_ptr).bits();

            let copy_bits = crate::object::ops_list::molt_list_copy(source_bits);
            let copy_ptr = obj_from_bits(copy_bits).as_ptr().expect("copy result");
            assert_eq!(unsafe { object_type_id(copy_ptr) }, TYPE_ID_LIST_INT);
            let copy = unsafe { crate::object::layout::list_int_vec_ref(copy_ptr) };
            assert_eq!(copy.as_slice(), &[2, 3, 5, 7]);

            dec_ref_bits(_py, copy_bits);
            dec_ref_bits(_py, source_bits);
        });
    }

    fn owner_count(bits: u64) -> u32 {
        unsafe {
            (*header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
        }
    }

    #[test]
    fn heap_fill_copy_slice_repeat_retain_original_owner() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let fill = int_bits_from_i64(py, 1_i64 << 62);
            let list = molt_list_int_new(MoltObject::from_int(3).bits(), fill);
            assert_eq!(
                unsafe { object_type_id(obj_from_bits(list).as_ptr().unwrap()) },
                TYPE_ID_LIST
            );
            let copy = crate::object::ops_list::molt_list_copy(list);
            let slice = molt_slice_new(
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
            let sliced = molt_list_int_getitem(list, slice);
            let repeated = crate::object::ops_arith::molt_mul(list, MoltObject::from_int(2).bits());
            assert!(!exception_pending(py));
            assert_eq!(
                owner_count(fill),
                16,
                "external owner and 3 + 3 + 3 + 6 list slots"
            );
            for container in [list, copy, sliced, repeated] {
                let later = molt_list_int_getitem(container, MoltObject::from_int(-1).bits());
                assert_eq!(later, fill);
                dec_ref_bits(py, later);
            }
            for bits in [repeated, sliced, slice, copy, list] {
                dec_ref_bits(py, bits);
            }
            assert_eq!(owner_count(fill), 1);
            dec_ref_bits(py, fill);
        });
    }

    #[test]
    fn raw_builder_family_boxes_occurrences_once_and_repeats_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let wide = 1_i64 << 62;
            let mut visited = Vec::new();
            let iter = crate::object::builders::alloc_list_int_from_raw_iter(py, 3, |index| {
                visited.push(index);
                [1, wide, wide][index]
            })
            .unwrap();
            assert_eq!(visited, [0, 1, 2]);
            assert_eq!(unsafe { object_type_id(iter) }, TYPE_ID_LIST);
            let repeated = crate::object::builders::alloc_list_int_from_repeated_raw_slice(
                py,
                &[wide, wide],
                2,
            )
            .unwrap();
            let bits = MoltObject::from_ptr(repeated).bits();
            let first = molt_list_int_getitem(bits, MoltObject::from_int(0).bits());
            let second = molt_list_int_getitem(bits, MoltObject::from_int(1).bits());
            let again = molt_list_int_getitem(bits, MoltObject::from_int(2).bits());
            assert_eq!(first, again);
            assert_ne!(
                first, second,
                "equal raw source occurrences acquire distinct owners"
            );
            assert_eq!(owner_count(first), 4, "two list slots and two read owners");
            for value in [
                first,
                second,
                again,
                bits,
                MoltObject::from_ptr(iter).bits(),
            ] {
                dec_ref_bits(py, value);
            }
            let filled = crate::object::builders::alloc_list_int_filled(py, 3, wide).unwrap();
            let bits = MoltObject::from_ptr(filled).bits();
            let first = molt_list_int_getitem(bits, MoltObject::from_int(0).bits());
            let last = molt_list_int_getitem(bits, MoltObject::from_int(2).bits());
            assert_eq!(first, last, "raw fill materializes one shared owner");
            for value in [first, last, bits] {
                dec_ref_bits(py, value);
            }
            let empty =
                crate::object::builders::alloc_list_int_from_repeated_raw_slice(py, &[wide], 0)
                    .unwrap();
            assert_eq!(unsafe { crate::list_len(empty) }, 0);
            dec_ref_bits(py, MoltObject::from_ptr(empty).bits());
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn promoted_storage_redispatches_boxed_access_and_rejects_raw_abi() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let fill = int_bits_from_i64(py, 1_i64 << 62);
            for boolean in [false, true] {
                let ptr = if boolean {
                    crate::object::builders::alloc_list_bool_from_raw_slice(py, &[0, 1]).unwrap()
                } else {
                    crate::object::builders::alloc_list_int_from_raw_slice(py, &[0, 1]).unwrap()
                };
                let bits = MoltObject::from_ptr(ptr).bits();
                let index = MoltObject::from_int(0).bits();
                if boolean {
                    molt_list_bool_setitem(bits, index, fill);
                } else {
                    molt_list_int_setitem(bits, index, fill);
                }
                assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_LIST);
                let read = if boolean {
                    molt_list_bool_getitem(bits, index)
                } else {
                    molt_list_int_getitem(bits, index)
                };
                assert_eq!(read, fill);
                dec_ref_bits(py, read);
                assert_eq!(molt_list_int_data(bits), 0);
                assert!(exception_pending(py));
                clear_exception(py);
                assert_eq!(molt_list_int_getitem_raw_checked(bits, 0), 0);
                assert!(exception_pending(py));
                clear_exception(py);
                dec_ref_bits(py, bits);
            }
            assert_eq!(owner_count(fill), 1);
            dec_ref_bits(py, fill);
        });
    }

    #[test]
    fn denied_promotion_keeps_original_flat_payload_and_owners() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let ptr = crate::object::builders::alloc_list_int_from_raw_slice(py, &[1, 2]).unwrap();
            let bits = MoltObject::from_ptr(ptr).bits();
            let fill = int_bits_from_i64(py, 1_i64 << 62);
            let data = molt_list_int_data(bits);
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            let reset = Reset;
            molt_list_int_setitem(bits, MoltObject::from_int(0).bits(), fill);
            assert!(exception_pending(py));
            drop(reset);
            clear_exception(py);
            assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_LIST_INT);
            assert_eq!(molt_list_int_data(bits), data);
            assert_eq!(molt_list_int_getitem_raw_checked(bits, 0), 1);
            assert_eq!(owner_count(fill), 1);
            dec_ref_bits(py, bits);
            dec_ref_bits(py, fill);
        });
    }
}
