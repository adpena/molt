//! Heap queue (heapq) operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_heapq_*` is a separate linker symbol.
//! Placing them in their own compilation unit lets `wasm-ld --gc-sections`
//! drop the entire block when no heapq builtins are referenced.

use crate::*;
use molt_obj_model::MoltObject;
use std::cmp::Ordering;

use super::ops::{CompareOutcome, compare_objects};

fn heapq_lt(_py: &PyToken<'_>, a_bits: u64, b_bits: u64) -> Option<bool> {
    let res_bits = super::ops_compare::molt_lt(a_bits, b_bits);
    if exception_pending(_py) {
        return None;
    }
    let truthy = is_truthy(_py, obj_from_bits(res_bits));
    let had_exc = exception_pending(_py);
    dec_ref_bits(_py, res_bits);
    if had_exc {
        return None;
    }
    Some(truthy)
}

unsafe fn heapq_siftdown(
    _py: &PyToken<'_>,
    heap: &mut [u64],
    startpos: usize,
    mut pos: usize,
) -> bool {
    let newitem = heap[pos];
    while pos > startpos {
        let parentpos = (pos - 1) / 2;
        let parent = heap[parentpos];
        let lt = match heapq_lt(_py, newitem, parent) {
            Some(val) => val,
            None => return false,
        };
        if lt {
            heap[pos] = parent;
            pos = parentpos;
            continue;
        }
        break;
    }
    heap[pos] = newitem;
    true
}

unsafe fn heapq_siftup(_py: &PyToken<'_>, heap: &mut [u64], mut pos: usize) -> bool {
    unsafe {
        let endpos = heap.len();
        let startpos = pos;
        let newitem = heap[pos];
        let mut childpos = 2 * pos + 1;
        while childpos < endpos {
            let rightpos = childpos + 1;
            if rightpos < endpos {
                let left_lt_right = match heapq_lt(_py, heap[childpos], heap[rightpos]) {
                    Some(val) => val,
                    None => return false,
                };
                if !left_lt_right {
                    childpos = rightpos;
                }
            }
            heap[pos] = heap[childpos];
            pos = childpos;
            childpos = 2 * pos + 1;
        }
        heap[pos] = newitem;
        heapq_siftdown(_py, heap, startpos, pos)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heapify(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            let len = elems.len();
            if len < 2 {
                return MoltObject::none().bits();
            }
            for idx in (0..len / 2).rev() {
                if !heapq_siftup(_py, elems, idx) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heappush(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            elems.push(item_bits);
            inc_ref_bits(_py, item_bits);
            let len = elems.len();
            if len > 1 && !heapq_siftdown(_py, elems, 0, len - 1) {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heappop(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                return raise_exception::<_>(_py, "IndexError", "index out of range");
            }
            let last = elems.pop().unwrap();
            if elems.is_empty() {
                inc_ref_bits(_py, last);
                dec_ref_bits(_py, last);
                return last;
            }
            let return_bits = elems[0];
            elems[0] = last;
            if !heapq_siftup(_py, elems, 0) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, return_bits);
            dec_ref_bits(_py, return_bits);
            return_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heapreplace(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                return raise_exception::<_>(_py, "IndexError", "index out of range");
            }
            let return_bits = elems[0];
            elems[0] = item_bits;
            inc_ref_bits(_py, item_bits);
            if !heapq_siftup(_py, elems, 0) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, return_bits);
            dec_ref_bits(_py, return_bits);
            return_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heappushpop(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                inc_ref_bits(_py, item_bits);
                return item_bits;
            }
            let lt = match heapq_lt(_py, elems[0], item_bits) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            if lt {
                let return_bits = elems[0];
                elems[0] = item_bits;
                inc_ref_bits(_py, item_bits);
                if !heapq_siftup(_py, elems, 0) {
                    return MoltObject::none().bits();
                }
                inc_ref_bits(_py, return_bits);
                dec_ref_bits(_py, return_bits);
                return return_bits;
            }
            inc_ref_bits(_py, item_bits);
            item_bits
        }
    })
}

// ---------------------------------------------------------------------------
// Max-heap internal helpers (reversed comparison: parent must be >= child)
// ---------------------------------------------------------------------------

/// Like `heapq_siftdown` but uses `parent < newitem` to maintain a max-heap
/// invariant (largest element at index 0).
unsafe fn heapq_siftdown_max(
    _py: &PyToken<'_>,
    heap: &mut [u64],
    startpos: usize,
    mut pos: usize,
) -> bool {
    let newitem = heap[pos];
    while pos > startpos {
        let parentpos = (pos - 1) / 2;
        let parent = heap[parentpos];
        // Max-heap: bubble up if parent < newitem (newitem is larger)
        let lt = match heapq_lt(_py, parent, newitem) {
            Some(val) => val,
            None => return false,
        };
        if lt {
            heap[pos] = parent;
            pos = parentpos;
            continue;
        }
        break;
    }
    heap[pos] = newitem;
    true
}

/// Like `heapq_siftup` but maintains a max-heap (largest element at root).
/// Pushes the root value down, always choosing the larger child.
unsafe fn heapq_siftup_max(_py: &PyToken<'_>, heap: &mut [u64], mut pos: usize) -> bool {
    unsafe {
        let endpos = heap.len();
        let startpos = pos;
        let newitem = heap[pos];
        let mut childpos = 2 * pos + 1;
        while childpos < endpos {
            let rightpos = childpos + 1;
            if rightpos < endpos {
                // Max-heap: choose the larger child (right if left < right)
                let left_lt_right = match heapq_lt(_py, heap[childpos], heap[rightpos]) {
                    Some(val) => val,
                    None => return false,
                };
                if left_lt_right {
                    childpos = rightpos;
                }
            }
            heap[pos] = heap[childpos];
            pos = childpos;
            childpos = 2 * pos + 1;
        }
        heap[pos] = newitem;
        heapq_siftdown_max(_py, heap, startpos, pos)
    }
}

// ---------------------------------------------------------------------------
// molt_heapq_heapify_max — build a max-heap in-place
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heapify_max(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            let len = elems.len();
            if len < 2 {
                return MoltObject::none().bits();
            }
            for idx in (0..len / 2).rev() {
                if !heapq_siftup_max(_py, elems, idx) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

// ---------------------------------------------------------------------------
// molt_heapq_heappop_max — pop the largest element from a max-heap
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_heappop_max(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                return raise_exception::<_>(_py, "IndexError", "index out of range");
            }
            let last = elems.pop().unwrap();
            if elems.is_empty() {
                // Only one element was in the heap; return it directly.
                // inc_ref + dec_ref transfers ownership back to caller.
                inc_ref_bits(_py, last);
                dec_ref_bits(_py, last);
                return last;
            }
            let return_bits = elems[0];
            elems[0] = last;
            if !heapq_siftup_max(_py, elems, 0) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, return_bits);
            dec_ref_bits(_py, return_bits);
            return_bits
        }
    })
}

// ---------------------------------------------------------------------------
// molt_heapq_nsmallest — return n smallest elements from an iterable
//
// Algorithm (mirrors CPython _heapq.c nsmallest):
//   - Materialise the iterable into a Vec<u64>.
//   - If n <= 0: return [].
//   - If n >= len: return sorted copy of the full list (ascending).
//   - Otherwise: build a max-heap of the first n elements, then for each
//     remaining element, if it is smaller than the heap root, replace the
//     root and sift down.  Finally sort the heap ascending.
//   - With a key function: maintain a parallel key array; comparisons use
//     the keys.
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_nsmallest(n_bits: u64, iterable_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // --- extract n ---
        let n_raw = index_i64_from_obj(_py, n_bits, "nsmallest() argument 'n' must be an integer");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if n_raw <= 0 {
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let n = n_raw as usize;

        // --- materialise iterable ---
        let src_bits = unsafe { list_from_iter_bits(_py, iterable_bits) };
        let Some(src_bits) = src_bits else {
            return MoltObject::none().bits();
        };

        let use_key = !obj_from_bits(key_bits).is_none();

        unsafe {
            let src_ptr = match obj_from_bits(src_bits).as_ptr() {
                Some(p) => p,
                None => {
                    dec_ref_bits(_py, src_bits);
                    return MoltObject::none().bits();
                }
            };
            let src = seq_vec_ref(src_ptr);
            let src_len = src.len();

            // Fast path: n >= len — sort a full copy.
            if n >= src_len {
                // Clone the elements into a Vec for sorting.
                let mut vals: Vec<u64> = src.to_vec();
                let mut keys: Vec<u64> = if use_key {
                    let mut ks = Vec::with_capacity(src_len);
                    for &v in vals.iter() {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for already in ks {
                                dec_ref_bits(_py, already);
                            }
                            return MoltObject::none().bits();
                        }
                        ks.push(k);
                    }
                    ks
                } else {
                    Vec::new()
                };

                // Sort ascending by key (or value when no key).
                let mut error: Option<()> = None;
                if use_key {
                    let keys_ptr = keys.as_mut_ptr();
                    let vals_ptr = vals.as_mut_ptr();
                    // Sort indices by key.
                    let mut indices: Vec<usize> = (0..src_len).collect();
                    indices.sort_by(|&ia, &ib| {
                        if error.is_some() {
                            return Ordering::Equal;
                        }
                        let ka = *keys_ptr.add(ia);
                        let kb = *keys_ptr.add(ib);
                        let outcome = compare_objects(_py, obj_from_bits(ka), obj_from_bits(kb));
                        match outcome {
                            CompareOutcome::Ordered(ord) => ord,
                            _ => {
                                error = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    if error.is_some() || exception_pending(_py) {
                        dec_ref_bits(_py, src_bits);
                        for k in keys {
                            dec_ref_bits(_py, k);
                        }
                        return MoltObject::none().bits();
                    }
                    let sorted_vals: Vec<u64> = indices.iter().map(|&i| *vals_ptr.add(i)).collect();
                    for k in keys {
                        dec_ref_bits(_py, k);
                    }
                    dec_ref_bits(_py, src_bits);
                    let out_ptr = alloc_list(_py, &sorted_vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                } else {
                    vals.sort_by(|&a, &b| {
                        if error.is_some() {
                            return Ordering::Equal;
                        }
                        match compare_objects(_py, obj_from_bits(a), obj_from_bits(b)) {
                            CompareOutcome::Ordered(ord) => ord,
                            _ => {
                                error = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    dec_ref_bits(_py, src_bits);
                    if error.is_some() || exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let out_ptr = alloc_list(_py, &vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                // --- heap-based path ---
                // heap_vals: the n candidate values.
                // heap_keys: parallel key array (only used when use_key).
                let mut heap_vals: Vec<u64> = Vec::with_capacity(n);
                let mut heap_keys: Vec<u64> = Vec::with_capacity(if use_key { n } else { 0 });

                // Fill heap with first n elements.
                for &v in src[..n].iter() {
                    heap_vals.push(v);
                    if use_key {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for k2 in heap_keys {
                                dec_ref_bits(_py, k2);
                            }
                            return MoltObject::none().bits();
                        }
                        heap_keys.push(k);
                    }
                }

                // Heapify the key array (or value array) as a max-heap.
                // We sift using the key slice when use_key, otherwise the val slice.
                if use_key {
                    let hlen = heap_keys.len();
                    for idx in (0..hlen / 2).rev() {
                        if !heapq_siftup_max(_py, &mut heap_keys, idx) {
                            dec_ref_bits(_py, src_bits);
                            for k in heap_keys {
                                dec_ref_bits(_py, k);
                            }
                            return MoltObject::none().bits();
                        }
                    }
                    // heap_keys is now a max-heap, but heap_vals is NOT in heap order.
                    // Re-build heap_vals in the same permutation as heap_keys using a
                    // sort + re-key approach: we need parallel tracking.
                    // Easier: rebuild from scratch keeping (key, val) pairs.
                    //
                    // Drop the separately built arrays and rebuild as a paired vec.
                    for k in heap_keys.drain(..) {
                        dec_ref_bits(_py, k);
                    }
                    // Re-fill as paired (key, val) max-heap on key.
                    let mut pairs: Vec<(u64, u64)> = Vec::with_capacity(n);
                    for &v in src[..n].iter() {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for (pk, _) in pairs {
                                dec_ref_bits(_py, pk);
                            }
                            return MoltObject::none().bits();
                        }
                        pairs.push((k, v));
                    }
                    // heapify pairs on key — max-heap.
                    // We need an inline sift that operates on pairs.
                    let plen = pairs.len();
                    for root in (0..plen / 2).rev() {
                        let mut pos = root;
                        loop {
                            let mut childpos = 2 * pos + 1;
                            if childpos >= plen {
                                break;
                            }
                            let rightpos = childpos + 1;
                            if rightpos < plen {
                                let left_lt_right =
                                    match heapq_lt(_py, pairs[childpos].0, pairs[rightpos].0) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            for (pk, _) in pairs {
                                                dec_ref_bits(_py, pk);
                                            }
                                            return MoltObject::none().bits();
                                        }
                                    };
                                if left_lt_right {
                                    childpos = rightpos;
                                }
                            }
                            // Max-heap: swap if child > parent
                            let child_lt_parent =
                                match heapq_lt(_py, pairs[childpos].0, pairs[pos].0) {
                                    Some(v) => v,
                                    None => {
                                        dec_ref_bits(_py, src_bits);
                                        for (pk, _) in pairs {
                                            dec_ref_bits(_py, pk);
                                        }
                                        return MoltObject::none().bits();
                                    }
                                };
                            if child_lt_parent {
                                break; // parent >= child, heap property satisfied
                            }
                            pairs.swap(pos, childpos);
                            pos = childpos;
                        }
                    }

                    // Process remaining elements.
                    for &v in src[n..].iter() {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for (pk, _) in pairs {
                                dec_ref_bits(_py, pk);
                            }
                            return MoltObject::none().bits();
                        }
                        // If v's key < heap root key, replace root.
                        let lt = match heapq_lt(_py, k, pairs[0].0) {
                            Some(val) => val,
                            None => {
                                dec_ref_bits(_py, k);
                                dec_ref_bits(_py, src_bits);
                                for (pk, _) in pairs {
                                    dec_ref_bits(_py, pk);
                                }
                                return MoltObject::none().bits();
                            }
                        };
                        if lt {
                            dec_ref_bits(_py, pairs[0].0);
                            pairs[0] = (k, v);
                            // Sift down the new root.
                            let plen2 = pairs.len();
                            let mut pos = 0usize;
                            loop {
                                let mut childpos = 2 * pos + 1;
                                if childpos >= plen2 {
                                    break;
                                }
                                let rightpos = childpos + 1;
                                if rightpos < plen2 {
                                    let left_lt_right =
                                        match heapq_lt(_py, pairs[childpos].0, pairs[rightpos].0) {
                                            Some(v) => v,
                                            None => {
                                                dec_ref_bits(_py, src_bits);
                                                for (pk, _) in pairs {
                                                    dec_ref_bits(_py, pk);
                                                }
                                                return MoltObject::none().bits();
                                            }
                                        };
                                    if left_lt_right {
                                        childpos = rightpos;
                                    }
                                }
                                let child_lt_parent =
                                    match heapq_lt(_py, pairs[childpos].0, pairs[pos].0) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            for (pk, _) in pairs {
                                                dec_ref_bits(_py, pk);
                                            }
                                            return MoltObject::none().bits();
                                        }
                                    };
                                if child_lt_parent {
                                    break;
                                }
                                pairs.swap(pos, childpos);
                                pos = childpos;
                            }
                        } else {
                            dec_ref_bits(_py, k);
                        }
                    }

                    // Sort pairs ascending by key and collect values.
                    let mut error2: Option<()> = None;
                    pairs.sort_by(|a, b| {
                        if error2.is_some() {
                            return Ordering::Equal;
                        }
                        match compare_objects(_py, obj_from_bits(a.0), obj_from_bits(b.0)) {
                            CompareOutcome::Ordered(ord) => ord,
                            _ => {
                                error2 = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    dec_ref_bits(_py, src_bits);
                    if error2.is_some() || exception_pending(_py) {
                        for (pk, _) in pairs {
                            dec_ref_bits(_py, pk);
                        }
                        return MoltObject::none().bits();
                    }
                    let out_vals: Vec<u64> = pairs.iter().map(|&(_, v)| v).collect();
                    for (pk, _) in pairs {
                        dec_ref_bits(_py, pk);
                    }
                    let out_ptr = alloc_list(_py, &out_vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                } else {
                    // No key function: work directly on values.
                    let hlen = heap_vals.len();
                    for idx in (0..hlen / 2).rev() {
                        if !heapq_siftup_max(_py, &mut heap_vals, idx) {
                            dec_ref_bits(_py, src_bits);
                            return MoltObject::none().bits();
                        }
                    }
                    // Process remaining elements.
                    for &v in src[n..].iter() {
                        let lt = match heapq_lt(_py, v, heap_vals[0]) {
                            Some(val) => val,
                            None => {
                                dec_ref_bits(_py, src_bits);
                                return MoltObject::none().bits();
                            }
                        };
                        if lt {
                            heap_vals[0] = v;
                            let hlen2 = heap_vals.len();
                            // Inline sift-down for max-heap.
                            let mut pos = 0usize;
                            loop {
                                let mut childpos = 2 * pos + 1;
                                if childpos >= hlen2 {
                                    break;
                                }
                                let rightpos = childpos + 1;
                                if rightpos < hlen2 {
                                    let left_lt_right = match heapq_lt(
                                        _py,
                                        heap_vals[childpos],
                                        heap_vals[rightpos],
                                    ) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            return MoltObject::none().bits();
                                        }
                                    };
                                    if left_lt_right {
                                        childpos = rightpos;
                                    }
                                }
                                let child_lt_parent =
                                    match heapq_lt(_py, heap_vals[childpos], heap_vals[pos]) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            return MoltObject::none().bits();
                                        }
                                    };
                                if child_lt_parent {
                                    break;
                                }
                                heap_vals.swap(pos, childpos);
                                pos = childpos;
                            }
                        }
                    }
                    dec_ref_bits(_py, src_bits);
                    // Sort the heap ascending before returning.
                    let mut error3: Option<()> = None;
                    heap_vals.sort_by(|&a, &b| {
                        if error3.is_some() {
                            return Ordering::Equal;
                        }
                        match compare_objects(_py, obj_from_bits(a), obj_from_bits(b)) {
                            CompareOutcome::Ordered(ord) => ord,
                            _ => {
                                error3 = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    if error3.is_some() || exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let out_ptr = alloc_list(_py, &heap_vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// molt_heapq_nlargest — return n largest elements from an iterable
//
// Algorithm (mirrors CPython _heapq.c nlargest):
//   - Materialise the iterable.
//   - If n <= 0: return [].
//   - If n >= len: return sorted copy descending.
//   - Otherwise: build a min-heap of the first n elements.  For remaining
//     elements, if element > heap[0] (min root), replace root and sift down.
//     Return heap sorted descending.
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_nlargest(n_bits: u64, iterable_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n_raw = index_i64_from_obj(_py, n_bits, "nlargest() argument 'n' must be an integer");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if n_raw <= 0 {
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let n = n_raw as usize;

        let src_bits = unsafe { list_from_iter_bits(_py, iterable_bits) };
        let Some(src_bits) = src_bits else {
            return MoltObject::none().bits();
        };

        let use_key = !obj_from_bits(key_bits).is_none();

        unsafe {
            let src_ptr = match obj_from_bits(src_bits).as_ptr() {
                Some(p) => p,
                None => {
                    dec_ref_bits(_py, src_bits);
                    return MoltObject::none().bits();
                }
            };
            let src = seq_vec_ref(src_ptr);
            let src_len = src.len();

            if n >= src_len {
                // Sort full copy descending.
                let mut vals: Vec<u64> = src.to_vec();
                let mut error: Option<()> = None;
                if use_key {
                    let mut keys: Vec<u64> = Vec::with_capacity(src_len);
                    for &v in vals.iter() {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for kk in keys {
                                dec_ref_bits(_py, kk);
                            }
                            return MoltObject::none().bits();
                        }
                        keys.push(k);
                    }
                    let keys_ptr = keys.as_mut_ptr();
                    let vals_ptr = vals.as_mut_ptr();
                    let mut indices: Vec<usize> = (0..src_len).collect();
                    indices.sort_by(|&ia, &ib| {
                        if error.is_some() {
                            return Ordering::Equal;
                        }
                        let ka = *keys_ptr.add(ia);
                        let kb = *keys_ptr.add(ib);
                        match compare_objects(_py, obj_from_bits(ka), obj_from_bits(kb)) {
                            CompareOutcome::Ordered(ord) => ord.reverse(), // descending
                            _ => {
                                error = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    if error.is_some() || exception_pending(_py) {
                        dec_ref_bits(_py, src_bits);
                        for kk in keys {
                            dec_ref_bits(_py, kk);
                        }
                        return MoltObject::none().bits();
                    }
                    let sorted_vals: Vec<u64> = indices.iter().map(|&i| *vals_ptr.add(i)).collect();
                    for kk in keys {
                        dec_ref_bits(_py, kk);
                    }
                    dec_ref_bits(_py, src_bits);
                    let out_ptr = alloc_list(_py, &sorted_vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                } else {
                    vals.sort_by(|&a, &b| {
                        if error.is_some() {
                            return Ordering::Equal;
                        }
                        match compare_objects(_py, obj_from_bits(a), obj_from_bits(b)) {
                            CompareOutcome::Ordered(ord) => ord.reverse(),
                            _ => {
                                error = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    dec_ref_bits(_py, src_bits);
                    if error.is_some() || exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let out_ptr = alloc_list(_py, &vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                // Min-heap of n candidates.
                if use_key {
                    // Use paired (key, val) min-heap.
                    let mut pairs: Vec<(u64, u64)> = Vec::with_capacity(n);
                    for &v in src[..n].iter() {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for (pk, _) in pairs {
                                dec_ref_bits(_py, pk);
                            }
                            return MoltObject::none().bits();
                        }
                        pairs.push((k, v));
                    }
                    // Heapify as min-heap on key.
                    let plen = pairs.len();
                    for root in (0..plen / 2).rev() {
                        let mut pos = root;
                        loop {
                            let mut childpos = 2 * pos + 1;
                            if childpos >= plen {
                                break;
                            }
                            let rightpos = childpos + 1;
                            if rightpos < plen {
                                // Min-heap: choose smaller child (right if left < right is false, i.e. right < left)
                                let right_lt_left =
                                    match heapq_lt(_py, pairs[rightpos].0, pairs[childpos].0) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            for (pk, _) in pairs {
                                                dec_ref_bits(_py, pk);
                                            }
                                            return MoltObject::none().bits();
                                        }
                                    };
                                if right_lt_left {
                                    childpos = rightpos;
                                }
                            }
                            let child_lt_parent =
                                match heapq_lt(_py, pairs[childpos].0, pairs[pos].0) {
                                    Some(v) => v,
                                    None => {
                                        dec_ref_bits(_py, src_bits);
                                        for (pk, _) in pairs {
                                            dec_ref_bits(_py, pk);
                                        }
                                        return MoltObject::none().bits();
                                    }
                                };
                            if !child_lt_parent {
                                break;
                            }
                            pairs.swap(pos, childpos);
                            pos = childpos;
                        }
                    }

                    // Process remaining: if new element's key > min root key, replace.
                    for &v in src[n..].iter() {
                        let k = call_callable1(_py, key_bits, v);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, src_bits);
                            for (pk, _) in pairs {
                                dec_ref_bits(_py, pk);
                            }
                            return MoltObject::none().bits();
                        }
                        // gt: heap root key < new key  =>  new element is larger
                        let root_lt_new = match heapq_lt(_py, pairs[0].0, k) {
                            Some(val) => val,
                            None => {
                                dec_ref_bits(_py, k);
                                dec_ref_bits(_py, src_bits);
                                for (pk, _) in pairs {
                                    dec_ref_bits(_py, pk);
                                }
                                return MoltObject::none().bits();
                            }
                        };
                        if root_lt_new {
                            dec_ref_bits(_py, pairs[0].0);
                            pairs[0] = (k, v);
                            // Sift down min-heap.
                            let plen2 = pairs.len();
                            let mut pos = 0usize;
                            loop {
                                let mut childpos = 2 * pos + 1;
                                if childpos >= plen2 {
                                    break;
                                }
                                let rightpos = childpos + 1;
                                if rightpos < plen2 {
                                    let right_lt_left =
                                        match heapq_lt(_py, pairs[rightpos].0, pairs[childpos].0) {
                                            Some(v) => v,
                                            None => {
                                                dec_ref_bits(_py, src_bits);
                                                for (pk, _) in pairs {
                                                    dec_ref_bits(_py, pk);
                                                }
                                                return MoltObject::none().bits();
                                            }
                                        };
                                    if right_lt_left {
                                        childpos = rightpos;
                                    }
                                }
                                let child_lt_parent =
                                    match heapq_lt(_py, pairs[childpos].0, pairs[pos].0) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            for (pk, _) in pairs {
                                                dec_ref_bits(_py, pk);
                                            }
                                            return MoltObject::none().bits();
                                        }
                                    };
                                if !child_lt_parent {
                                    break;
                                }
                                pairs.swap(pos, childpos);
                                pos = childpos;
                            }
                        } else {
                            dec_ref_bits(_py, k);
                        }
                    }

                    // Sort descending by key.
                    let mut error2: Option<()> = None;
                    pairs.sort_by(|a, b| {
                        if error2.is_some() {
                            return Ordering::Equal;
                        }
                        match compare_objects(_py, obj_from_bits(a.0), obj_from_bits(b.0)) {
                            CompareOutcome::Ordered(ord) => ord.reverse(),
                            _ => {
                                error2 = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    dec_ref_bits(_py, src_bits);
                    if error2.is_some() || exception_pending(_py) {
                        for (pk, _) in pairs {
                            dec_ref_bits(_py, pk);
                        }
                        return MoltObject::none().bits();
                    }
                    let out_vals: Vec<u64> = pairs.iter().map(|&(_, v)| v).collect();
                    for (pk, _) in pairs {
                        dec_ref_bits(_py, pk);
                    }
                    let out_ptr = alloc_list(_py, &out_vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                } else {
                    // No key: min-heap directly on values.
                    let mut heap_vals: Vec<u64> = src[..n].to_vec();
                    let hlen = heap_vals.len();
                    for idx in (0..hlen / 2).rev() {
                        if !heapq_siftup(_py, &mut heap_vals, idx) {
                            dec_ref_bits(_py, src_bits);
                            return MoltObject::none().bits();
                        }
                    }
                    for &v in src[n..].iter() {
                        let root_lt_v = match heapq_lt(_py, heap_vals[0], v) {
                            Some(val) => val,
                            None => {
                                dec_ref_bits(_py, src_bits);
                                return MoltObject::none().bits();
                            }
                        };
                        if root_lt_v {
                            heap_vals[0] = v;
                            let hlen2 = heap_vals.len();
                            // Inline min-heap sift-down.
                            let mut pos = 0usize;
                            loop {
                                let mut childpos = 2 * pos + 1;
                                if childpos >= hlen2 {
                                    break;
                                }
                                let rightpos = childpos + 1;
                                if rightpos < hlen2 {
                                    let right_lt_left = match heapq_lt(
                                        _py,
                                        heap_vals[rightpos],
                                        heap_vals[childpos],
                                    ) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            return MoltObject::none().bits();
                                        }
                                    };
                                    if right_lt_left {
                                        childpos = rightpos;
                                    }
                                }
                                let child_lt_parent =
                                    match heapq_lt(_py, heap_vals[childpos], heap_vals[pos]) {
                                        Some(v) => v,
                                        None => {
                                            dec_ref_bits(_py, src_bits);
                                            return MoltObject::none().bits();
                                        }
                                    };
                                if !child_lt_parent {
                                    break;
                                }
                                heap_vals.swap(pos, childpos);
                                pos = childpos;
                            }
                        }
                    }
                    dec_ref_bits(_py, src_bits);
                    let mut error3: Option<()> = None;
                    heap_vals.sort_by(|&a, &b| {
                        if error3.is_some() {
                            return Ordering::Equal;
                        }
                        match compare_objects(_py, obj_from_bits(a), obj_from_bits(b)) {
                            CompareOutcome::Ordered(ord) => ord.reverse(),
                            _ => {
                                error3 = Some(());
                                Ordering::Equal
                            }
                        }
                    });
                    if error3.is_some() || exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let out_ptr = alloc_list(_py, &heap_vals);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// molt_heapq_merge — k-way merge of sorted iterables
//
// iterables_bits: a list of lists (each already sorted in the appropriate order)
// key_bits:       None or a callable used for comparison
// reverse_bits:   bool — if True, inputs are in descending order and output
//                 is also descending (merge as if reversed)
//
// Algorithm: use a heap of (current_key, index_into_source_list,
// current_position_in_source_list) to drive a classic k-way merge.
// Because Molt uses materialised (not lazy) iterables, we load all source
// lists up front and build the result list in memory.
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_heapq_merge(iterables_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = obj_from_bits(iterables_bits);
        let Some(iter_ptr) = iter_obj.as_ptr() else {
            // Empty iterables argument → return empty list.
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(ptr).bits();
        };

        let use_key = !obj_from_bits(key_bits).is_none();
        let reverse = is_truthy(_py, obj_from_bits(reverse_bits));

        unsafe {
            if object_type_id(iter_ptr) != TYPE_ID_LIST {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "heapq.merge: iterables must be a list of iterables",
                );
            }

            // Materialise each source iterable into its own Vec<u64>.
            let outer = seq_vec_ref(iter_ptr);
            let k = outer.len();
            if k == 0 {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                return MoltObject::from_ptr(ptr).bits();
            }

            // sources[i] = materialised Vec of raw bits for iterable i.
            // We borrow values from the already-allocated list objects — no
            // extra inc_ref needed here because we don't outlive src_bits.
            let mut source_lists: Vec<u64> = Vec::with_capacity(k); // holds list object bits
            for &it_bits in outer.iter() {
                let mat = list_from_iter_bits(_py, it_bits);
                let Some(mat_bits) = mat else {
                    for prev in source_lists {
                        dec_ref_bits(_py, prev);
                    }
                    return MoltObject::none().bits();
                };
                source_lists.push(mat_bits);
            }

            // positions[i] = current index into source_lists[i].
            let mut positions: Vec<usize> = vec![0usize; k];

            // Heap entry: (key_bits, source_index, value_bits).
            // We own key_bits (inc_ref'd when pushed; dec_ref'd when popped).
            // value_bits is borrowed from the source list — we inc_ref it when
            // we emit it into the output.
            struct MergeEntry {
                key_bits: u64,
                src_idx: usize,
                val_bits: u64,
            }

            // Comparison for the merge heap.
            // For forward merge (reverse=false): min-heap on key (smallest first).
            // For reverse merge (reverse=true):  max-heap on key (largest first).
            macro_rules! entry_lt {
                ($py:expr, $a:expr, $b:expr, $rev:expr) => {{
                    if $rev {
                        // max-heap: $a < $b  ⟺  $b should be popped first
                        // We want the LARGEST at the root, so use the same lt
                        // but swapped for the heap ordering:
                        // entry_lt!(a,b,true) returns true when b < a (a is larger)
                        heapq_lt($py, $b.key_bits, $a.key_bits)
                    } else {
                        heapq_lt($py, $a.key_bits, $b.key_bits)
                    }
                }};
            }

            // Helper: sift the heap up (bubble newly pushed entry up).
            macro_rules! sift_up_heap {
                ($py:expr, $heap:expr, $rev:expr) => {{
                    let mut ok = true;
                    let mut pos = $heap.len() - 1;
                    while pos > 0 {
                        let parent = (pos - 1) / 2;
                        match entry_lt!($py, $heap[pos], $heap[parent], $rev) {
                            None => {
                                ok = false;
                                break;
                            }
                            Some(true) => {
                                $heap.swap(pos, parent);
                                pos = parent;
                            }
                            Some(false) => break,
                        }
                    }
                    ok
                }};
            }

            // Helper: sift the heap down (fix root after replacement).
            macro_rules! sift_down_heap {
                ($py:expr, $heap:expr, $rev:expr) => {{
                    let mut ok = true;
                    let endpos = $heap.len();
                    let mut pos = 0usize;
                    loop {
                        let mut childpos = 2 * pos + 1;
                        if childpos >= endpos {
                            break;
                        }
                        let rightpos = childpos + 1;
                        if rightpos < endpos {
                            match entry_lt!($py, $heap[rightpos], $heap[childpos], $rev) {
                                None => {
                                    ok = false;
                                    break;
                                }
                                Some(true) => childpos = rightpos,
                                Some(false) => {}
                            }
                        }
                        match entry_lt!($py, $heap[childpos], $heap[pos], $rev) {
                            None => {
                                ok = false;
                                break;
                            }
                            Some(true) => {
                                $heap.swap(pos, childpos);
                                pos = childpos;
                            }
                            Some(false) => break,
                        }
                    }
                    ok
                }};
            }

            // Helper to free all heap entries and source lists.
            macro_rules! cleanup {
                ($py:expr, $heap:expr, $sources:expr) => {
                    for e in $heap.drain(..) {
                        if use_key {
                            dec_ref_bits($py, e.key_bits);
                        }
                        // val_bits is borrowed; no dec_ref here.
                    }
                    for sl in $sources.drain(..) {
                        dec_ref_bits($py, sl);
                    }
                };
            }

            // Seed the heap with the first element of each non-empty source.
            let mut heap: Vec<MergeEntry> = Vec::with_capacity(k);
            for (src_idx, &sl_bits) in source_lists.iter().enumerate() {
                let sl_ptr = match obj_from_bits(sl_bits).as_ptr() {
                    Some(p) => p,
                    None => continue,
                };
                let sl = seq_vec_ref(sl_ptr);
                if sl.is_empty() {
                    continue;
                }
                let val_bits = sl[0];
                positions[src_idx] = 1;
                let key_bits_entry = if use_key {
                    let k_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        cleanup!(_py, heap, source_lists);
                        return MoltObject::none().bits();
                    }
                    k_bits
                } else {
                    val_bits
                };
                heap.push(MergeEntry {
                    key_bits: key_bits_entry,
                    src_idx,
                    val_bits,
                });
                if !sift_up_heap!(_py, heap, reverse) {
                    if use_key && !heap.is_empty() {
                        // dec_ref the key we just pushed (it's at some position in heap now)
                    }
                    cleanup!(_py, heap, source_lists);
                    return MoltObject::none().bits();
                }
            }

            // Drain the heap into the output list.
            let mut out: Vec<u64> = Vec::new();
            while !heap.is_empty() {
                // Pop minimum (root).
                let top = heap.swap_remove(0);
                if !heap.is_empty() && !sift_down_heap!(_py, heap, reverse) {
                    if use_key {
                        dec_ref_bits(_py, top.key_bits);
                    }
                    cleanup!(_py, heap, source_lists);
                    return MoltObject::none().bits();
                }
                // inc_ref the value before adding to output.
                inc_ref_bits(_py, top.val_bits);
                out.push(top.val_bits);
                if use_key {
                    dec_ref_bits(_py, top.key_bits);
                }

                // Advance the source iterator and push next entry if available.
                let src_idx = top.src_idx;
                let sl_ptr = match obj_from_bits(source_lists[src_idx]).as_ptr() {
                    Some(p) => p,
                    None => continue,
                };
                let sl = seq_vec_ref(sl_ptr);
                let pos = positions[src_idx];
                if pos < sl.len() {
                    let next_val = sl[pos];
                    positions[src_idx] = pos + 1;
                    let next_key = if use_key {
                        let k_bits = call_callable1(_py, key_bits, next_val);
                        if exception_pending(_py) {
                            // dec_ref any values already added to out
                            for v in out.drain(..) {
                                dec_ref_bits(_py, v);
                            }
                            cleanup!(_py, heap, source_lists);
                            return MoltObject::none().bits();
                        }
                        k_bits
                    } else {
                        next_val
                    };
                    heap.push(MergeEntry {
                        key_bits: next_key,
                        src_idx,
                        val_bits: next_val,
                    });
                    if !sift_up_heap!(_py, heap, reverse) {
                        for v in out.drain(..) {
                            dec_ref_bits(_py, v);
                        }
                        cleanup!(_py, heap, source_lists);
                        return MoltObject::none().bits();
                    }
                }
            }

            // Free source lists.
            for sl in source_lists {
                dec_ref_bits(_py, sl);
            }

            // Build result list.  out already contains inc_ref'd values.
            // alloc_list inc_refs again, so we need to dec_ref our own refs.
            let out_ptr = alloc_list(_py, &out);
            // Release our own references (alloc_list took its own).
            for v in out {
                dec_ref_bits(_py, v);
            }
            if out_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
