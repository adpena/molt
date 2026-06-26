use super::*;

fn list_specialized_index_from_bits(index_bits: u64) -> Option<i64> {
    let index_obj = obj_from_bits(index_bits);
    if let Some(i) = to_i64(index_obj) {
        return Some(i);
    }
    crate::with_gil_entry_nopanic!(_py, {
        let key = obj_from_bits(index_bits);
        let type_err = format!(
            "list indices must be integers or slices, not {}",
            type_name(_py, key)
        );
        index_i64_with_overflow(_py, index_bits, &type_err, None)
    })
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

unsafe fn list_int_slice_to_boxed_list(_py: &PyToken<'_>, ptr: *mut u8, slice_ptr: *mut u8) -> u64 {
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
        let mut out: Vec<u64>;
        if step == 1 {
            let s = start as usize;
            let mut e = stop as usize;
            if s > e {
                e = s;
            }
            out = Vec::with_capacity(e.saturating_sub(s));
            for raw in elems.iter().skip(s).take(e.saturating_sub(s)) {
                out.push(MoltObject::from_int(*raw).bits());
            }
        } else {
            let indices = collect_slice_indices(start, stop, step);
            out = Vec::with_capacity(indices.len());
            for idx in indices {
                out.push(MoltObject::from_int(elems[idx]).bits());
            }
        }
        let out_ptr = alloc_list_with_capacity_owned(_py, &out, out.len());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

unsafe fn list_bool_slice_to_boxed_list(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    slice_ptr: *mut u8,
) -> u64 {
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
        let mut out: Vec<u64>;
        if step == 1 {
            let s = start as usize;
            let mut e = stop as usize;
            if s > e {
                e = s;
            }
            out = Vec::with_capacity(e.saturating_sub(s));
            for raw in elems.iter().skip(s).take(e.saturating_sub(s)) {
                out.push(MoltObject::from_bool(*raw != 0).bits());
            }
        } else {
            let indices = collect_slice_indices(start, stop, step);
            out = Vec::with_capacity(indices.len());
            for idx in indices {
                out.push(MoltObject::from_bool(elems[idx] != 0).bits());
            }
        }
        let out_ptr = alloc_list_with_capacity_owned(_py, &out, out.len());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem(list_bits: u64, index_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        let index_obj = obj_from_bits(index_bits);
        if let Some(slice_ptr) = index_obj.as_ptr()
            && object_type_id(slice_ptr) == TYPE_ID_SLICE
        {
            return crate::with_gil_entry_nopanic!(_py, {
                list_int_slice_to_boxed_list(_py, ptr, slice_ptr)
            });
        }
        let Some(mut idx) = list_specialized_index_from_bits(index_bits) else {
            return MoltObject::none().bits();
        };
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return list_index_out_of_range_error();
        }
        let raw_val = *storage.data.add(idx as usize);
        MoltObject::from_int(raw_val).bits()
    }
}

/// Raw-register fast path for list[int] getitem.
/// Takes a raw i64 index (NOT NaN-boxed) and returns a raw i64 value (NOT NaN-boxed).
/// Eliminates NaN-box/unbox round-trips when both index and result stay in raw_int_shadow.
/// Returns 0 on out-of-bounds (matching Python's behavior for sieve-like patterns where
/// the caller checks truthiness — 0 is falsy).
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_raw(list_bits: u64, raw_index: i64) -> i64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        let mut idx = raw_index;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return 0;
        }
        *storage.data.add(idx as usize)
    }
}

/// Raw-register list[int] getitem with Python exception semantics.
///
/// Takes a raw i64 index and returns the raw i64 element. On out-of-bounds it
/// raises the same IndexError as the boxed getitem path and returns 0 only as
/// an exception-continuation sentinel.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_raw_checked(list_bits: u64, raw_index: i64) -> i64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        let mut idx = raw_index;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            let _ = list_index_out_of_range_error();
            return 0;
        }
        *storage.data.add(idx as usize)
    }
}

/// Ultra-fast list[int] getitem: no bounds check, no negative index handling.
/// Used when the compiler can prove the index is non-negative and in bounds
/// (e.g., loop counter bounded by list length).
///
/// # Safety
/// Caller must guarantee `0 <= raw_index < len(list)`.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_list_int_getitem_unchecked(list_bits: u64, raw_index: i64) -> i64 {
    unsafe {
        let ptr = obj_from_bits(list_bits).as_ptr().unwrap_unchecked();
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        *storage.data.add(raw_index as usize)
    }
}

/// Ultra-fast list[int] setitem — no bounds check, no negative index handling.
///
/// # Safety
/// Caller must guarantee `0 <= raw_index < len(list)`.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_list_int_setitem_unchecked(
    list_bits: u64,
    raw_index: i64,
    raw_value: i64,
) -> u64 {
    unsafe {
        let ptr = obj_from_bits(list_bits).as_ptr().unwrap_unchecked();
        let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
        *storage.data.add(raw_index as usize) = raw_value;
    }
    list_bits
}

/// Raw-register fast path for list[int] setitem.
/// Takes raw i64 index and value (NOT NaN-boxed). Stores value directly into the flat i64 array.
/// Returns list_bits unchanged (matching molt_list_int_setitem contract).
/// No-op on out-of-bounds.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem_raw(list_bits: u64, raw_index: i64, raw_value: i64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return list_bits;
    };
    unsafe {
        let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        let mut idx = raw_index;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return list_bits;
        }
        *storage.data.add(idx as usize) = raw_value;
        list_bits
    }
}

/// GIL-free list[int] getitem with NaN-boxed interface.
///
/// Identical to `molt_list_int_getitem` (which already skips GIL), but named
/// `_nogil` to make the contract explicit for the compiler backend.
/// No GIL acquisition, no catch_unwind, no signal checks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_nogil(list_bits: u64, index_bits: u64) -> u64 {
    molt_list_int_getitem(list_bits, index_bits)
}

/// GIL-free list[int] setitem with NaN-boxed interface.
///
/// Identical to `molt_list_int_setitem` (which already skips GIL), but named
/// `_nogil` to make the contract explicit for the compiler backend.
/// No GIL acquisition, no catch_unwind, no signal checks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem_nogil(
    list_bits: u64,
    index_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_list_int_setitem(list_bits, index_bits, value_bits)
}

/// Set element in a specialized list[int].
/// Expects a NaN-boxed int value — extracts raw i64 and stores directly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::none().bits();
    };
    let index_obj = obj_from_bits(index_bits);
    if let Some(slice_ptr) = index_obj.as_ptr()
        && unsafe { object_type_id(slice_ptr) == TYPE_ID_SLICE }
    {
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
            }
        });
        return molt_store_index(list_bits, index_bits, value_bits);
    }
    let value_obj = obj_from_bits(value_bits);
    if !value_obj.is_int() {
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
            }
        });
        return molt_store_index(list_bits, index_bits, value_bits);
    }
    unsafe {
        let Some(mut idx) = list_specialized_index_from_bits(index_bits) else {
            return MoltObject::none().bits();
        };
        let raw_value = value_obj.as_int_unchecked();
        let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return list_assignment_out_of_range_error();
        }
        *storage.data.add(idx as usize) = raw_value;
        list_bits
    }
}

/// Get element from a specialized list[bool].
/// Returns a NaN-boxed bool (True or False).
/// No refcounting needed -- bools are inline NaN-boxed values.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_bool_getitem(list_bits: u64, index_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        let index_obj = obj_from_bits(index_bits);
        if let Some(slice_ptr) = index_obj.as_ptr()
            && object_type_id(slice_ptr) == TYPE_ID_SLICE
        {
            return crate::with_gil_entry_nopanic!(_py, {
                list_bool_slice_to_boxed_list(_py, ptr, slice_ptr)
            });
        }
        let Some(mut idx) = list_specialized_index_from_bits(index_bits) else {
            return MoltObject::none().bits();
        };
        let storage = &*crate::object::layout::list_bool_storage_ptr(ptr);
        let len = storage.len as i64;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return list_index_out_of_range_error();
        }
        let raw_val = *storage.data.add(idx as usize);
        MoltObject::from_bool(raw_val != 0).bits()
    }
}

/// Set element in a specialized list[bool].
/// Accepts NaN-boxed bool or int value -- converts to u8 (0 or 1).
/// No refcounting needed -- bools are inline NaN-boxed values.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_bool_setitem(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::none().bits();
    };
    let index_obj = obj_from_bits(index_bits);
    if let Some(slice_ptr) = index_obj.as_ptr()
        && unsafe { object_type_id(slice_ptr) == TYPE_ID_SLICE }
    {
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
            }
        });
        return molt_store_index(list_bits, index_bits, value_bits);
    }
    let value_obj = obj_from_bits(value_bits);
    let Some(value_bool) = value_obj.as_bool() else {
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
            }
        });
        return molt_store_index(list_bits, index_bits, value_bits);
    };
    unsafe {
        let Some(mut idx) = list_specialized_index_from_bits(index_bits) else {
            return MoltObject::none().bits();
        };
        let raw_value: u8 = if value_bool { 1 } else { 0 };
        let storage = &mut *crate::object::layout::list_bool_storage_ptr(ptr);
        let len = storage.len as i64;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return list_assignment_out_of_range_error();
        }
        *storage.data.add(idx as usize) = raw_value;
        list_bits
    }
}

/// Return the raw data pointer of a list (regular or list_int).
///
/// For list_int: reads from `ListIntStorage.data` (`#[repr(C)]`, offset 0).
/// For regular lists: reads from `Vec<u64>.as_ptr()`.
/// The returned pointer is valid only as long as the list is not resized.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_data(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        // Check if this is a list_int (ListIntStorage) or regular list (Vec<u64>)
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        storage.data as u64
    }
}

/// Return the length of a list (regular or list_int) as a raw u64.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_len_raw(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        storage.len as u64
    }
}

/// Get length of a specialized list[int].
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_len(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::from_int(0).bits();
    };
    unsafe {
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        MoltObject::from_int(storage.len as i64).bits()
    }
}

/// Check if value is truthy in a specialized list[int] element context.
/// Raw i64: 0 is falsy, everything else is truthy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_truthy(list_bits: u64, index_bits: u64) -> u64 {
    let index_obj = obj_from_bits(index_bits);
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::from_bool(false).bits();
    };
    unsafe {
        let mut idx = if index_obj.is_int() {
            index_obj.as_int_unchecked()
        } else {
            return MoltObject::from_bool(false).bits();
        };
        let storage = &*crate::object::layout::list_int_storage_ptr(ptr);
        let len = storage.len as i64;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return MoltObject::from_bool(false).bits();
        }
        let raw_val = *storage.data.add(idx as usize);
        MoltObject::from_bool(raw_val != 0).bits()
    }
}

/// Fast path: integer index store into a list (STORE_SUBSCR_LIST_INT).
///
/// On any failure falls through to the full `molt_store_index` slow path.
/// Returns the container bits on success (matching `molt_store_index`),
/// or `MoltObject::none().bits()` on error.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_setitem_int_fast(
    list_bits: u64,
    index_bits: u64,
    val_bits: u64,
) -> u64 {
    // 1. Fast tag check: index must be a NaN-boxed int.
    let index_obj = obj_from_bits(index_bits);
    if !index_obj.is_int() {
        return molt_store_index(list_bits, index_bits, val_bits);
    }
    // 2. List must be a heap pointer.
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return molt_store_index(list_bits, index_bits, val_bits);
    };
    unsafe {
        // 3. Must actually be a list (regular or specialized).
        let tid = object_type_id(ptr);
        if tid == TYPE_ID_LIST_BOOL {
            // list[bool] fast path — delegate to specialized setitem.
            return molt_list_bool_setitem(list_bits, index_bits, val_bits);
        }
        if tid != TYPE_ID_LIST {
            return molt_store_index(list_bits, index_bits, val_bits);
        }
        // 4. Extract index and list length.
        let mut idx = index_obj.as_int_unchecked();
        let len = list_len(ptr) as i64;
        // 5. Handle negative indexing.
        if idx < 0 {
            idx += len;
        }
        // 6. Bounds check — fall through to slow path which raises IndexError.
        if idx < 0 || idx >= len {
            return molt_store_index(list_bits, index_bits, val_bits);
        }
        // 7. Direct array store with reference count update.
        crate::with_gil_entry_nopanic!(_py, {
            let elems = seq_vec(ptr);
            let old_bits = elems[idx as usize];
            if old_bits != val_bits {
                // Skip refcount ops for inline primitives (bool, int, None, float).
                // These are NaN-boxed values with no heap allocation — as_ptr()
                // returns None so inc_ref_bits/dec_ref_bits would be no-ops, but
                // skipping the function call eliminates overhead in hot loops
                // (e.g., sieve: is_prime[i] = False millions of times).
                if crate::object::refcount_opt::is_heap_ref(val_bits) {
                    inc_ref_bits(_py, val_bits);
                    (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
                }
                elems[idx as usize] = val_bits;
                if crate::object::refcount_opt::is_heap_ref(old_bits) {
                    dec_ref_bits(_py, old_bits);
                }
            }
            list_bits
        })
    }
}

/// Unchecked list getitem — used when BCE (Bounds Check Elimination) has proven
/// the index is in bounds.
///
/// # Safety
/// The caller guarantees:
///   - `list_bits` is a valid NaN-boxed heap pointer to a TYPE_ID_LIST object.
///   - `0 <= index < len(list)` — no bounds check is performed.
///   - The list is not mutated concurrently (GIL must be held by the caller).
///
/// Violating any of these preconditions causes undefined behaviour.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_getitem_unchecked(list_bits: u64, index: i64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    // Safety: caller guarantees list_bits is a valid list heap pointer.
    let ptr = unsafe { list_obj.as_ptr().unwrap_unchecked() };
    unsafe {
        let elems = seq_vec_ref(ptr);
        // Safety: caller guarantees 0 <= index < len.
        let val = *elems.get_unchecked(index as usize);
        crate::with_gil_entry_nopanic!(_py, {
            inc_ref_bits(_py, val);
            val
        })
    }
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
        let elems = seq_vec_ref(ptr);
        let len = elems.len() as i64;
        // 5. Handle negative indexing.
        if idx < 0 {
            idx += len;
        }
        // 6. Bounds check.
        if idx < 0 || idx >= len {
            return molt_index(list_bits, index_bits);
        }
        // 7. Direct array load and reference-count increment.
        // Skip with_gil_entry! — compiled code already holds the GIL and
        // inc_ref is just an atomic fetch_add that cannot panic. Eliminating
        // catch_unwind saves ~15ns per list access in hot loops.
        let val = elems[idx as usize];
        let val_obj = obj_from_bits(val);
        if let Some(val_ptr) = val_obj.as_ptr() {
            let header = val_ptr.sub(std::mem::size_of::<crate::object::MoltHeader>())
                as *mut crate::object::MoltHeader;
            if ((*header).flags & crate::object::HEADER_FLAG_IMMORTAL) == 0 {
                (*header)
                    .ref_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
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
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        // Not a pointer — fall back to generic path by boxing the index
        return molt_list_getitem_int_fast(list_bits, MoltObject::from_int(raw_idx).bits());
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_LIST {
            return molt_list_getitem_int_fast(list_bits, MoltObject::from_int(raw_idx).bits());
        }
        let mut idx = raw_idx;
        let elems = seq_vec_ref(ptr);
        let len = elems.len() as i64;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            // Out of bounds — fall back to generic path which raises IndexError
            return molt_list_getitem_int_fast(list_bits, MoltObject::from_int(raw_idx).bits());
        }
        let val = elems[idx as usize];
        let val_obj = obj_from_bits(val);
        if let Some(val_ptr) = val_obj.as_ptr() {
            let header = val_ptr.sub(std::mem::size_of::<crate::object::MoltHeader>())
                as *mut crate::object::MoltHeader;
            if ((*header).flags & crate::object::HEADER_FLAG_IMMORTAL) == 0 {
                (*header)
                    .ref_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        val
    }
}

/// List setitem with a raw i64 index.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_list_setitem_raw_idx(list_bits: u64, raw_idx: i64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(ptr) = list_obj.as_ptr() else {
            return molt_list_setitem_int_fast(
                list_bits,
                MoltObject::from_int(raw_idx).bits(),
                val_bits,
            );
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                return molt_list_setitem_int_fast(
                    list_bits,
                    MoltObject::from_int(raw_idx).bits(),
                    val_bits,
                );
            }
            let mut idx = raw_idx;
            let elems = &mut *seq_vec_ptr(ptr);
            let len = elems.len() as i64;
            if idx < 0 {
                idx += len;
            }
            if idx < 0 || idx >= len {
                // Out of bounds — fall back to generic path which raises IndexError
                return molt_list_setitem_int_fast(
                    list_bits,
                    MoltObject::from_int(raw_idx).bits(),
                    val_bits,
                );
            }
            // Dec-ref old value, store new, inc-ref new.
            // Skip refcount ops for inline primitives (bool, int, None, float)
            // — they have no heap allocation, so inc/dec_ref_bits are no-ops.
            let old = elems[idx as usize];
            elems[idx as usize] = val_bits;
            if crate::object::refcount_opt::is_heap_ref(val_bits) {
                inc_ref_bits(_py, val_bits);
                (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
            }
            if crate::object::refcount_opt::is_heap_ref(old) {
                dec_ref_bits(_py, old);
            }
            MoltObject::none().bits()
        }
    })
}

// ── Specialized list[int] operations ────────────────────────────────
//
// When the compiler proves a list contains only integers, it uses these
// specialized functions that store raw i64 values without NaN-boxing.
// Element access is a single array load + box_int on return.
// No refcounting needed (ints are NaN-boxed inline, not heap-allocated).

/// Allocate a specialized list[int] with raw i64 storage.
/// Elements are stored as raw i64 (NOT NaN-boxed).
/// Returns a NaN-boxed pointer to the list object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_new(count: u64, fill_value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Both arguments are NaN-boxed — unbox the count
        let count_obj = obj_from_bits(count);
        let n = if count_obj.is_int() {
            let v = count_obj.as_int_unchecked();
            if v < 0 { 0usize } else { v as usize }
        } else if count_obj.is_bool() {
            if count_obj.as_bool().unwrap_or(false) {
                1
            } else {
                0
            }
        } else {
            return MoltObject::none().bits();
        };
        // Extract raw int from the NaN-boxed fill value
        let fill_obj = obj_from_bits(fill_value);
        let fill_raw = if fill_obj.is_none() {
            0i64
        } else if fill_obj.is_int() {
            fill_obj.as_int_unchecked()
        } else if fill_obj.is_bool() {
            if fill_obj.as_bool().unwrap_or(false) {
                1i64
            } else {
                0i64
            }
        } else {
            // Not an int — fall back to regular list
            return MoltObject::none().bits();
        };

        let total = std::mem::size_of::<crate::object::MoltHeader>()
            + std::mem::size_of::<*mut crate::object::layout::ListIntStorage>()
            + std::mem::size_of::<u64>(); // padding
        let ptr = alloc_object(_py, total, TYPE_ID_LIST_INT);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let Some(storage_ptr) = crate::object::layout::ListIntStorage::filled(n, fill_raw)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
            };
            *(ptr as *mut *mut crate::object::layout::ListIntStorage) = storage_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_fill_new(count: u64, fill_value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let count_obj = obj_from_bits(count);
        let n = if let Some(v) = count_obj.as_int() {
            if v < 0 { 0usize } else { v as usize }
        } else if count_obj.is_bool() {
            if count_obj.as_bool().unwrap_or(false) {
                1usize
            } else {
                0usize
            }
        } else {
            return MoltObject::none().bits();
        };

        let total = std::mem::size_of::<crate::object::MoltHeader>()
            + std::mem::size_of::<*mut crate::object::DataclassDesc>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<u64>();
        let ptr = alloc_object(_py, total, TYPE_ID_LIST);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let Some(vec_ptr) = crate::object::backing::tracked_vec_box_with_capacity::<u64>(n)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
            };
            (*vec_ptr).resize(n, fill_value);
            *(ptr as *mut *mut Vec<u64>) = vec_ptr;
            if let Some(fill_ptr) = obj_from_bits(fill_value).as_ptr() {
                let mut remaining = n;
                while remaining > 0 {
                    let batch = remaining.min(u32::MAX as usize) as u32;
                    crate::object::inc_ref_n_ptr(_py, fill_ptr, batch);
                    remaining -= batch as usize;
                }
                (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
