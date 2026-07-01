// Arithmetic, bitwise, and percent-format operations.
// Split from ops.rs for compilation-unit size reduction.
#![allow(clippy::items_after_test_module)]

use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};

use super::ops::{
    as_float_extended, call_binary_dunder, call_inplace_dunder, concat_bytes_like,
    fill_repeated_bytes, float_result_bits, is_float_extended,
};

mod percent_format;

use percent_format::string_percent_format_impl;

fn is_number_for_concat(obj: MoltObject) -> bool {
    if is_float_extended(obj) {
        return true;
    }
    if to_i64(obj).is_some() {
        return true;
    }
    if bigint_ptr_from_bits(obj.bits()).is_some() {
        return true;
    }
    if complex_ptr_from_bits(obj.bits()).is_some() {
        return true;
    }
    false
}

/// Fast string concatenation for known-str operands.
/// Skips the 8-branch type dispatch in `molt_add`.
/// Falls back to `molt_add` if either operand is not actually a string
/// (defensive: the frontend may have mis-inferred the type hint).
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_concat(lhs_bits: u64, rhs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(lhs_bits);
        let rhs = obj_from_bits(rhs_bits);
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                if object_type_id(lp) == TYPE_ID_STRING && object_type_id(rp) == TYPE_ID_STRING {
                    let l_len = string_len(lp);
                    let r_len = string_len(rp);
                    let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_STRING) {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        // Fallback: type hint was wrong, delegate to full dispatch.
        molt_add(lhs_bits, rhs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Note: exception_pending check removed — backends guarantee molt_add
        // is only called on non-exception paths, so the TLS + atomic overhead
        // of checking every arithmetic op is unnecessary.
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // BigInt fast path — hoisted above inline int/float checks because
        // BigInt operands fail every is_int/is_float/to_i64 check before
        // reaching the pointer dispatch, wasting ~8 branch mispredictions
        // per call.  In tight BigInt loops (e.g. fib(1_000_000)) this path
        // dominates, so checking it first avoids all that overhead.
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            let ltype = unsafe { object_type_id(lp) };
            let rtype = unsafe { object_type_id(rp) };
            if ltype == TYPE_ID_BIGINT && rtype == TYPE_ID_BIGINT {
                let l_ref = unsafe { bigint_ref(lp) };
                let r_ref = unsafe { bigint_ref(rp) };
                let res: BigInt = l_ref + r_ref;
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
        }
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 -> 2).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            let res = li as i128 + ri as i128;
            return int_bits_from_i128(_py, res);
        }
        // Float fast path — second most common after int, moved before
        // as_ptr / bigint checks to avoid unnecessary pointer dereferences.
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return float_result_bits(_py, lf + rf);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                // BigInt+BigInt already handled above (hoisted fast path).
                if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
                    let l_len = string_len(lp);
                    let r_len = string_len(rp);
                    let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_STRING) {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
                if ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTES {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_BYTES) {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
                if ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTEARRAY {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_BYTEARRAY)
                    {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
                if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
                    let l_len = list_len(lp);
                    let r_len = list_len(rp);
                    let l_elems = seq_vec_ref(lp);
                    let r_elems = seq_vec_ref(rp);
                    let mut combined = Vec::with_capacity(l_len + r_len);
                    combined.extend_from_slice(l_elems);
                    combined.extend_from_slice(r_elems);
                    let ptr = alloc_list(_py, &combined);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                if ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE {
                    let l_len = tuple_len(lp);
                    let r_len = tuple_len(rp);
                    let l_elems = seq_vec_ref(lp);
                    let r_elems = seq_vec_ref(rp);
                    let mut combined = Vec::with_capacity(l_len + r_len);
                    combined.extend_from_slice(l_elems);
                    combined.extend_from_slice(r_elems);
                    let ptr = alloc_tuple(_py, &combined);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                // Mixed BigInt + inline int: promote inline to BigInt ref-add
                if ltype == TYPE_ID_BIGINT
                    && let Some(ri) = to_i64(rhs)
                {
                    let l_ref = bigint_ref(lp);
                    let res: BigInt = l_ref + BigInt::from(ri);
                    if let Some(i) = bigint_to_inline(&res) {
                        return MoltObject::from_int(i).bits();
                    }
                    return bigint_bits(_py, res);
                }
                if rtype == TYPE_ID_BIGINT
                    && let Some(li) = to_i64(lhs)
                {
                    let r_ref = bigint_ref(rp);
                    let res: BigInt = BigInt::from(li) + r_ref;
                    if let Some(i) = bigint_to_inline(&res) {
                        return MoltObject::from_int(i).bits();
                    }
                    return bigint_bits(_py, res);
                }
            }
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big + r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    return complex_bits(_py, lc.re + rc.re, lc.im + rc.im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        unsafe {
            let add_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.add_name, b"__add__");
            let radd_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.radd_name, b"__radd__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, add_name_bits, radd_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "+")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concat(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if is_number_for_concat(lhs) && is_number_for_concat(rhs) {
            return binary_type_error(_py, lhs, rhs, "+");
        }
        molt_add(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                let ltype = object_type_id(ptr);
                if ltype == TYPE_ID_LIST || ltype == TYPE_ID_LIST_BOOL || ltype == TYPE_ID_LIST_INT
                {
                    let _ = molt_list_extend(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
                if ltype == TYPE_ID_STRING {
                    // In-place string concat: O(n) amortised when refcount == 1.
                    let header = &mut *header_from_obj_ptr(ptr);
                    if header.ref_count.load(std::sync::atomic::Ordering::Relaxed) == 1
                        && (header.flags & crate::object::HEADER_FLAG_IMMORTAL) == 0
                    {
                        let rhs_obj = obj_from_bits(b);
                        if let Some(r_ptr) = rhs_obj.as_ptr()
                            && object_type_id(r_ptr) == TYPE_ID_STRING
                        {
                            let l_len = string_len(ptr);
                            let r_len = string_len(r_ptr);
                            if let Some(content_len) = l_len.checked_add(r_len) {
                                let needed = std::mem::size_of::<MoltHeader>()
                                    + std::mem::size_of::<usize>()
                                    + content_len;
                                let total_sz = super::total_size_from_header(header, ptr);
                                if total_sz >= needed {
                                    // Fast: spare capacity — append in place, zero alloc
                                    let l_data = string_bytes(ptr) as *mut u8;
                                    let r_data = string_bytes(r_ptr);
                                    std::ptr::copy_nonoverlapping(r_data, l_data.add(l_len), r_len);
                                    *(ptr as *mut usize) = l_len + r_len;
                                    super::object_set_state(ptr, 0); // invalidate hash
                                    inc_ref_bits(_py, a);
                                    return a;
                                }
                                // Slow: allocate 2x, amortised growth
                                let new_cap = std::cmp::max(total_sz * 2, needed + 64);
                                let new_ptr = alloc_object(_py, new_cap, TYPE_ID_STRING);
                                if !new_ptr.is_null() {
                                    let l_data = string_bytes(ptr);
                                    let r_data = string_bytes(r_ptr);
                                    let n_data = string_bytes(new_ptr) as *mut u8;
                                    std::ptr::copy_nonoverlapping(l_data, n_data, l_len);
                                    std::ptr::copy_nonoverlapping(r_data, n_data.add(l_len), r_len);
                                    *(new_ptr as *mut usize) = l_len + r_len;
                                    // Caller dec-refs old LHS after storing result.
                                    return MoltObject::from_ptr(new_ptr).bits();
                                }
                            } // if let Some(content_len) — overflow falls through
                        }
                    }
                    // Fall through to regular add (concat)
                }
                if ltype == TYPE_ID_BYTEARRAY {
                    if bytearray_concat_in_place(_py, ptr, b) {
                        inc_ref_bits(_py, a);
                        return a;
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        unsafe {
            let iadd_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.iadd_name, b"__iadd__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, iadd_name_bits) {
                return res_bits;
            }
        }
        molt_add(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_concat(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if is_number_for_concat(lhs) && is_number_for_concat(rhs) {
            return binary_type_error(_py, lhs, rhs, "+");
        }
        molt_inplace_add(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sub(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // BigInt fast path — hoisted above inline int/float checks to avoid
        // wasted branch mispredictions when both operands are heap BigInts.
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            let ltype = unsafe { object_type_id(lp) };
            let rtype = unsafe { object_type_id(rp) };
            if ltype == TYPE_ID_BIGINT && rtype == TYPE_ID_BIGINT {
                let l_ref = unsafe { bigint_ref(lp) };
                let r_ref = unsafe { bigint_ref(rp) };
                let res: BigInt = l_ref - r_ref;
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
        }
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            let res = li as i128 - ri as i128;
            return int_bits_from_i128(_py, res);
        }
        // Float fast path — moved before bigint/as_ptr checks.
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return float_result_bits(_py, lf - rf);
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big - r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    return complex_bits(_py, lc.re - rc.re, lc.im - rc.im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_difference(_py, lp, rp, ltype);
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let (rhs_ptr, rhs_bits) = if is_set_like_type(rtype) {
                        (rp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, rp, rtype) else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_difference(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
            }
        }
        unsafe {
            let sub_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.sub_name, b"__sub__");
            let rsub_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rsub_name, b"__rsub__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, sub_name_bits, rsub_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "-")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_sub(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Int/float fast paths — avoid dunder dispatch overhead for numeric types.
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            return int_bits_from_i128(_py, li as i128 - ri as i128);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return float_result_bits(_py, lf - rf);
        }
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "-=", a, b);
                    }
                    let _ = molt_set_difference_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        unsafe {
            let isub_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.isub_name, b"__isub__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, isub_name_bits) {
                return res_bits;
            }
        }
        molt_sub(a, b)
    })
}

pub(crate) fn repeat_sequence(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> Option<u64> {
    unsafe {
        let type_id = object_type_id(ptr);
        if count <= 0 {
            let out_ptr = match type_id {
                TYPE_ID_LIST | TYPE_ID_LIST_BOOL | TYPE_ID_LIST_INT => alloc_list(_py, &[]),
                TYPE_ID_TUPLE => alloc_tuple(_py, &[]),
                TYPE_ID_STRING => alloc_string(_py, &[]),
                TYPE_ID_BYTES => alloc_bytes(_py, &[]),
                TYPE_ID_BYTEARRAY => alloc_bytearray(_py, &[]),
                _ => return None,
            };
            if out_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return Some(MoltObject::from_ptr(out_ptr).bits());
        }
        if count == 1 && type_id == TYPE_ID_TUPLE {
            let bits = MoltObject::from_ptr(ptr).bits();
            inc_ref_bits(_py, bits);
            return Some(bits);
        }

        let times = count as usize;
        match type_id {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(ptr);
                let total = match elems.len().checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                if elems.len() == 1 {
                    let val = elems[0];
                    let val_obj = obj_from_bits(val);

                    // Bool fast path: [True] * N or [False] * N → ListBoolStorage.
                    // Uses u8 backing (1 byte per element) instead of u64 (8 bytes),
                    // giving 8x cache-line density for iteration-heavy patterns like
                    // sieve of Eratosthenes.
                    if val_obj.is_bool() {
                        let fill: u8 = if val_obj.as_bool().unwrap_or(false) {
                            1
                        } else {
                            0
                        };
                        let Some(storage_ptr) =
                            crate::object::layout::ListBoolStorage::filled(total, fill)
                        else {
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "list allocation failed",
                            );
                        };
                        let obj_size = std::mem::size_of::<crate::object::MoltHeader>()
                            + std::mem::size_of::<*mut crate::object::layout::ListBoolStorage>()
                            + std::mem::size_of::<u64>(); // padding
                        let out_ptr = alloc_object(_py, obj_size, TYPE_ID_LIST_BOOL);
                        if out_ptr.is_null() {
                            // Reconstruct and drop the vec to free the buffer.
                            drop((*Box::from_raw(storage_ptr)).into_vec());
                            return raise_exception::<_>(_py, "MemoryError", "out of memory");
                        }
                        *(out_ptr as *mut *mut crate::object::layout::ListBoolStorage) =
                            storage_ptr;
                        return Some(MoltObject::from_ptr(out_ptr).bits());
                    }

                    // Int fast path: [0] * N, [42] * N, [-1] * N → ListIntStorage.
                    // Uses flat i64 backing store (no NaN-boxing per element),
                    // enabling direct memory loads in the native backend's inline
                    // getitem/setitem paths.
                    if let Some(int_val) = val_obj.as_int() {
                        let Some(storage_ptr) =
                            crate::object::layout::ListIntStorage::filled(total, int_val)
                        else {
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "list allocation failed",
                            );
                        };
                        let obj_size = std::mem::size_of::<crate::object::MoltHeader>()
                            + std::mem::size_of::<*mut crate::object::layout::ListIntStorage>()
                            + std::mem::size_of::<u64>(); // padding
                        let out_ptr = alloc_object(_py, obj_size, TYPE_ID_LIST_INT);
                        if out_ptr.is_null() {
                            // Reconstruct and drop the vec to free the buffer.
                            drop((*Box::from_raw(storage_ptr)).into_vec());
                            return raise_exception::<_>(_py, "MemoryError", "out of memory");
                        }
                        *(out_ptr as *mut *mut crate::object::layout::ListIntStorage) = storage_ptr;
                        return Some(MoltObject::from_ptr(out_ptr).bits());
                    }

                    // Single-element repeat: vec![val; total] compiles to
                    // memset-like fill — O(n) memory writes, zero per-element
                    // function calls.
                    let combined = vec![val; total];
                    // Use _owned variant: we handle refcounting ourselves in
                    // batch instead of N individual inc_ref_bits calls.
                    let out_ptr = alloc_list_with_capacity_owned(_py, &combined, total);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    // Batch refcount: one atomic add instead of `total` adds.
                    // For non-pointer values (bools, ints, floats, None),
                    // is_heap_ref is false so we skip entirely — zero atomics.
                    if crate::object::refcount_opt::is_heap_ref(val) {
                        let obj_ptr = MoltObject::from_bits(val).as_ptr().unwrap();
                        let header = header_from_obj_ptr(obj_ptr);
                        if ((*header).flags & crate::object::HEADER_FLAG_IMMORTAL) == 0 {
                            (*header)
                                .ref_count
                                .fetch_add(total as u32, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Some(MoltObject::from_ptr(out_ptr).bits())
                } else {
                    let mut combined = Vec::with_capacity(total);
                    for _ in 0..times {
                        combined.extend_from_slice(elems);
                    }
                    let out_ptr = alloc_list(_py, &combined);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    Some(MoltObject::from_ptr(out_ptr).bits())
                }
            }
            TYPE_ID_LIST_BOOL => {
                let elems = crate::object::layout::list_bool_vec_ref(ptr);
                let src = elems.as_slice();
                let Some(storage_ptr) =
                    crate::object::layout::ListBoolStorage::repeated_slice(src, times)
                else {
                    return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
                };
                let obj_size = std::mem::size_of::<crate::object::MoltHeader>()
                    + std::mem::size_of::<*mut crate::object::layout::ListBoolStorage>()
                    + std::mem::size_of::<u64>();
                let out_ptr = alloc_object(_py, obj_size, TYPE_ID_LIST_BOOL);
                if out_ptr.is_null() {
                    drop((*Box::from_raw(storage_ptr)).into_vec());
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                *(out_ptr as *mut *mut crate::object::layout::ListBoolStorage) = storage_ptr;
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_LIST_INT => {
                let elems = crate::object::layout::list_int_vec_ref(ptr);
                let Some(storage_ptr) =
                    crate::object::layout::ListIntStorage::repeated_slice(elems.as_slice(), times)
                else {
                    return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
                };
                let obj_size = std::mem::size_of::<crate::object::MoltHeader>()
                    + std::mem::size_of::<*mut crate::object::layout::ListIntStorage>()
                    + std::mem::size_of::<u64>();
                let out_ptr = alloc_object(_py, obj_size, TYPE_ID_LIST_INT);
                if out_ptr.is_null() {
                    drop((*Box::from_raw(storage_ptr)).into_vec());
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                *(out_ptr as *mut *mut crate::object::layout::ListIntStorage) = storage_ptr;
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(ptr);
                let total = match elems.len().checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                if elems.len() == 1 {
                    let val = elems[0];
                    let combined = vec![val; total];
                    let out_ptr = alloc_tuple_with_capacity_owned(_py, &combined, total);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if crate::object::refcount_opt::is_heap_ref(val) {
                        let obj_ptr = MoltObject::from_bits(val).as_ptr().unwrap();
                        let header = header_from_obj_ptr(obj_ptr);
                        if ((*header).flags & crate::object::HEADER_FLAG_IMMORTAL) == 0 {
                            (*header)
                                .ref_count
                                .fetch_add(total as u32, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Some(MoltObject::from_ptr(out_ptr).bits())
                } else {
                    let mut combined = Vec::with_capacity(total);
                    for _ in 0..times {
                        combined.extend_from_slice(elems);
                    }
                    let out_ptr = alloc_tuple(_py, &combined);
                    if out_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    Some(MoltObject::from_ptr(out_ptr).bits())
                }
            }
            TYPE_ID_STRING => {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let total = match len.checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let out_ptr = alloc_bytes_like_with_len(_py, total, TYPE_ID_STRING);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_BYTES => {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let total = match len.checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let out_ptr = alloc_bytes_like_with_len(_py, total, TYPE_ID_BYTES);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_BYTEARRAY => {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let total = match len.checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let mut out = Vec::with_capacity(total);
                for _ in 0..times {
                    out.extend_from_slice(bytes);
                }
                let out_ptr = alloc_bytearray(_py, &out);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            _ => None,
        }
    }
}

unsafe fn list_repeat_in_place(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> bool {
    unsafe {
        let vec_ptr = seq_vec_ptr(ptr);
        let elems = &mut *vec_ptr;
        if count <= 0 {
            for &item in elems.iter() {
                dec_ref_bits(_py, item);
            }
            elems.clear();
            return true;
        }
        let count = match usize::try_from(count) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if count == 1 {
            return true;
        }
        let snapshot = elems.clone();
        if snapshot.is_empty() {
            return true;
        }
        let total = match snapshot.len().checked_mul(count) {
            Some(total) => total,
            None => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if !crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            vec_ptr,
            total,
            "list allocation failed",
        ) {
            return false;
        }
        for _ in 1..count {
            for &item in snapshot.iter() {
                elems.push(item);
                inc_ref_bits(_py, item);
            }
        }
        true
    }
}

unsafe fn bytearray_repeat_in_place(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> bool {
    unsafe {
        let vec_ptr = bytearray_vec_ptr(ptr);
        let elems = &mut *vec_ptr;
        if count <= 0 {
            elems.clear();
            return true;
        }
        let count = match usize::try_from(count) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if count == 1 {
            return true;
        }
        let snapshot = elems.clone();
        if snapshot.is_empty() {
            return true;
        }
        let total = match snapshot.len().checked_mul(count) {
            Some(total) => total,
            None => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if !crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            vec_ptr,
            total,
            "bytearray allocation failed",
        ) {
            return false;
        }
        for _ in 1..count {
            elems.extend_from_slice(&snapshot);
        }
        true
    }
}

unsafe fn bytearray_concat_in_place(_py: &PyToken<'_>, ptr: *mut u8, other_bits: u64) -> bool {
    unsafe {
        let other = obj_from_bits(other_bits);
        let Some(other_ptr) = other.as_ptr() else {
            let msg = format!("can't concat {} to bytearray", type_name(_py, other));
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let other_type = object_type_id(other_ptr);
        let payload = if other_type == TYPE_ID_MEMORYVIEW {
            if memoryview_released(other_ptr) {
                return raise_released_memoryview(_py);
            }
            if let Some(slice) = memoryview_bytes_slice(other_ptr) {
                slice.to_vec()
            } else if let Some(buf) = memoryview_collect_bytes(other_ptr) {
                buf
            } else {
                let msg = format!("can't concat {} to bytearray", type_name(_py, other));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        } else if other_type == TYPE_ID_BYTES || other_type == TYPE_ID_BYTEARRAY {
            if other_ptr == ptr {
                bytearray_vec_ref(ptr).clone()
            } else {
                bytes_like_slice_raw(other_ptr).unwrap_or(&[]).to_vec()
            }
        } else {
            let msg = format!("can't concat {} to bytearray", type_name(_py, other));
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let vec_ptr = bytearray_vec_ptr(ptr);
        let elems = &mut *vec_ptr;
        let Some(required_len) = elems.len().checked_add(payload.len()) else {
            return raise_exception::<_>(_py, "MemoryError", "bytearray allocation failed");
        };
        if !crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            vec_ptr,
            required_len,
            "bytearray allocation failed",
        ) {
            return false;
        }
        elems.extend_from_slice(&payload);
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Int/float fast paths — avoid dunder dispatch overhead for numeric types.
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            return int_bits_from_i128(_py, li as i128 * ri as i128);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return float_result_bits(_py, lf * rf);
        }
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                let ltype = object_type_id(ptr);
                if ltype == TYPE_ID_LIST
                    || ltype == TYPE_ID_LIST_BOOL
                    || ltype == TYPE_ID_LIST_INT
                    || ltype == TYPE_ID_BYTEARRAY
                {
                    let rhs_type = type_name(_py, obj_from_bits(b));
                    let msg = format!("can't multiply sequence by non-int of type '{rhs_type}'");
                    let count = index_i64_from_obj(_py, b, &msg);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let ok = if ltype == TYPE_ID_LIST {
                        list_repeat_in_place(_py, ptr, count)
                    } else if ltype == TYPE_ID_LIST_BOOL || ltype == TYPE_ID_LIST_INT {
                        // Promote specialized list to regular list, then repeat in-place.
                        crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
                        list_repeat_in_place(_py, ptr, count)
                    } else {
                        bytearray_repeat_in_place(_py, ptr, count)
                    };
                    if !ok || exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        // Try `__imul__` before the binary fallback (CPython parity). This was
        // missing — `x *= y` skipped the in-place dunder for user objects and
        // went straight to `__mul__`/`__rmul__` (the same bug class fixed for
        // //=/**=/etc.; `molt_inplace_add`/`molt_inplace_sub` already do this).
        unsafe {
            let imul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.imul_name, b"__imul__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, imul_name_bits) {
                return res_bits;
            }
        }
        molt_mul(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // BigInt fast path — hoisted to avoid wasted to_i64/is_float checks
        // when both operands are heap BigInts.
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            let ltype = unsafe { object_type_id(lp) };
            let rtype = unsafe { object_type_id(rp) };
            if ltype == TYPE_ID_BIGINT && rtype == TYPE_ID_BIGINT {
                let l_ref = unsafe { bigint_ref(lp) };
                let r_ref = unsafe { bigint_ref(rp) };
                let res: BigInt = l_ref * r_ref;
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
        }
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            let res = li as i128 * ri as i128;
            return int_bits_from_i128(_py, res);
        }
        // Float fast path — moved before repeat_sequence/bigint checks.
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return float_result_bits(_py, lf * rf);
        }
        if let Some(count) = to_i64(lhs)
            && let Some(ptr) = rhs.as_ptr()
            && let Some(bits) = repeat_sequence(_py, ptr, count)
        {
            return bits;
        }
        if let Some(count) = to_i64(rhs)
            && let Some(ptr) = lhs.as_ptr()
            && let Some(bits) = repeat_sequence(_py, ptr, count)
        {
            return bits;
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big * r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    let re = lc.re * rc.re - lc.im * rc.im;
                    let im = lc.im * rc.re + lc.re * rc.im;
                    return complex_bits(_py, re, im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        unsafe {
            let mul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.mul_name, b"__mul__");
            let rmul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rmul_name, b"__rmul__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, mul_name_bits, rmul_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "*")
    })
}

/// Correctly-rounded integer true division `num / den` returning the nearest
/// IEEE-754 double, matching CPython's `long_true_divide` (`Objects/longobject.c`).
///
/// Both operands are exact arbitrary-precision integers and `den` must be
/// non-zero (the caller handles division by zero). Returns `None` when the
/// magnitude of the correctly-rounded result exceeds `f64::MAX`, which the
/// caller maps to `OverflowError` exactly as CPython does.
///
/// A naive `num.to_f64() / den.to_f64()` double-rounds (each operand is first
/// rounded to a double, then divided), disagreeing with CPython by up to one ULP
/// once an operand exceeds 2^53. This instead produces a single correctly-rounded
/// result: it scales so the exact integer quotient retains `DBL_MANT_DIG + 2`
/// significant bits plus a sticky bit derived from the exact remainder, then lets
/// the hardware `u64 -> f64` conversion apply IEEE-754 round-half-to-even before
/// scaling by the recorded power of two via `ldexp`.
fn bigint_true_divide(num: &BigInt, den: &BigInt) -> Option<f64> {
    // IEEE-754 binary64 parameters (mirrors C's float.h for the CPython port).
    const DBL_MANT_DIG: i64 = 53;
    const DBL_MAX_EXP: i64 = 1024;
    const DBL_MIN_EXP: i64 = -1021;
    // Quotient bits extracted beyond the 53-bit mantissa: one guard bit plus one
    // sticky bit, matching CPython's `extra_bits = 2`.
    const EXTRA_BITS: i64 = 2;

    debug_assert!(!den.is_zero());
    if num.is_zero() {
        return Some(0.0);
    }

    let negate = (num.sign() == Sign::Minus) != (den.sign() == Sign::Minus);
    // Work with non-negative magnitudes; the sign is reapplied at the end.
    let a = BigInt::from(num.magnitude().clone());
    let b = BigInt::from(den.magnitude().clone());
    let a_bits = a.bits() as i64; // bit_length of |num| (>= 1)
    let b_bits = b.bits() as i64; // bit_length of |den| (>= 1)

    // `diff` brackets the result exponent: 2^(diff-1) <= |num|/|den| < 2^(diff+1).
    let diff = a_bits - b_bits;
    if diff > DBL_MAX_EXP {
        return None; // certain overflow
    }
    if diff < DBL_MIN_EXP - DBL_MANT_DIG - 1 {
        return Some(if negate { -0.0 } else { 0.0 }); // certain underflow to 0
    }

    // Scale so the integer quotient keeps DBL_MANT_DIG + EXTRA_BITS significant
    // bits, clamped at the subnormal floor so we never demand more precision than
    // a subnormal double provides.
    let shift = std::cmp::max(diff, DBL_MIN_EXP) - DBL_MANT_DIG - EXTRA_BITS;

    // q = floor(|num| / |den| * 2^-shift), with `inexact` set when a non-zero
    // remainder was discarded. `shift <= 0` scales the numerator up; otherwise it
    // scales the denominator up. All arithmetic is exact BigInt arithmetic.
    let (mut quotient, inexact) = if shift <= 0 {
        let scaled = &a << ((-shift) as u64);
        let (q, r) = scaled.div_rem(&b);
        (q, !r.is_zero())
    } else {
        let scaled_den = &b << (shift as u64);
        let (q, r) = a.div_rem(&scaled_den);
        (q, !r.is_zero())
    };

    // Fold the discarded remainder into the least-significant bit as a sticky bit
    // so the hardware mantissa rounding sees "round up past a half" correctly
    // (CPython's `if (inexact) x |= 1`). Only sets the bit when it is currently
    // clear, preserving the value's parity for ties-to-even.
    if inexact && !quotient.bit(0) {
        quotient += 1;
    }

    // `quotient` has at most DBL_MANT_DIG + EXTRA_BITS + 1 = 56 bits, so it fits a
    // u64. The `u64 -> f64` cast rounds half-to-even (IEEE-754 default), then
    // `ldexp` scales by 2^shift without further rounding.
    let q_u: u64 = quotient
        .to_u64()
        .expect("scaled true-divide quotient must fit in 56 bits");
    let scaled = (q_u as f64) * (shift as f64).exp2();
    if scaled.is_infinite() {
        return None; // rounding pushed the result past f64::MAX
    }
    Some(if negate { -scaled } else { scaled })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { div_impl(_py, a, b, "/") })
}

/// True-division core. `err_op` is the operator symbol used for the terminal
/// "unsupported operand type(s)" TypeError — `/` for `molt_div`, `/=` for the
/// in-place path (`molt_inplace_div`), matching CPython's `op_name` threading
/// (`binary_op1`/`binary_iop1`). Every other (numeric / ZeroDivision / dunder)
/// outcome is symbol-independent and identical for both spellings.
fn div_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Python true division: int / int always returns float
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            if ri == 0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
            }
            return float_result_bits(_py, li as f64 / ri as f64);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
            }
            return float_result_bits(_py, lf / rf);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    let denom = rc.re * rc.re + rc.im * rc.im;
                    if denom == 0.0 {
                        return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
                    }
                    let re = (lc.re * rc.re + lc.im * rc.im) / denom;
                    let im = (lc.im * rc.re - lc.re * rc.im) / denom;
                    return complex_bits(_py, re, im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        // Integer true division where at least one operand is a BigInt that does
        // not fit i64 (the `to_i64` fast path above could not handle it). Python
        // `int / int` always yields a correctly-rounded float; emit it via the
        // CPython `long_true_divide` algorithm. Both operands must be integer
        // (int / bool / BigInt); a non-numeric object falls through to dunder
        // dispatch. Floats and complexes are already handled above, so `to_bigint`
        // here only matches genuine integers.
        if (bigint_ptr_from_bits(a).is_some() || bigint_ptr_from_bits(b).is_some())
            && let (Some(la), Some(lb)) = (to_bigint(lhs), to_bigint(rhs))
        {
            if lb.is_zero() {
                return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
            }
            match bigint_true_divide(&la, &lb) {
                Some(q) => return float_result_bits(_py, q),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "integer division result too large for a float",
                    );
                }
            }
        }
        unsafe {
            let div_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.truediv_name,
                b"__truediv__",
            );
            let rdiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.rtruediv_name,
                b"__rtruediv__",
            );
            if let Some(res_bits) = call_binary_dunder(_py, a, b, div_name_bits, rdiv_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, err_op)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let idiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.itruediv_name,
                b"__itruediv__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, idiv_name_bits) {
                return res_bits;
            }
        }
        // Binary fallback with the AUGMENTED symbol so a final TypeError reads
        // `unsupported operand type(s) for /=:` (CPython parity), not `for /:`.
        div_impl(_py, a, b, "/=")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { floordiv_impl(_py, a, b, "//") })
}

/// Floor-division core. `err_op` selects the terminal TypeError symbol (`//`
/// vs `//=`); all other outcomes are spelling-independent.
fn floordiv_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let either_float = lhs.is_float() || rhs.is_float();
        if !either_float && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            if li == i64::MIN && ri == -1 {
                // overflow — fall through to bigint
            } else {
                let q = li / ri;
                let r = li % ri;
                let res = if r != 0 && (r < 0) != (ri < 0) {
                    q - 1
                } else {
                    q
                };
                return int_bits_from_i64(_py, res);
            }
        }
        if !either_float && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let res = l_big.div_floor(&r_big);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "float floor division by zero",
                );
            }
            return float_result_bits(_py, (lf / rf).floor());
        }
        unsafe {
            let div_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.floordiv_name,
                b"__floordiv__",
            );
            let rdiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.rfloordiv_name,
                b"__rfloordiv__",
            );
            if let Some(res_bits) = call_binary_dunder(_py, a, b, div_name_bits, rdiv_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, err_op)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let idiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.ifloordiv_name,
                b"__ifloordiv__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, idiv_name_bits) {
                return res_bits;
            }
        }
        floordiv_impl(_py, a, b, "//=")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { mod_impl(_py, a, b, "%") })
}

/// Modulo / `%`-format core. `err_op` selects the terminal TypeError symbol
/// (`%` vs `%=`); the `%`-string-format path and numeric paths are
/// spelling-independent.
fn mod_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Int fast path first — much more common than string % formatting.
        // Skip if either operand is a float so that e.g. 7 % 2.0 returns 1.0 (float).
        let either_float = lhs.is_float() || rhs.is_float();
        if !either_float && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let mut rem = li % ri;
            if rem != 0 && (rem > 0) != (ri > 0) {
                rem += ri;
            }
            return MoltObject::from_int(rem).bits();
        }
        // String % formatting — moved after int fast path.
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    let text = string_obj_to_owned(lhs).unwrap_or_default();
                    let Some(rendered) = string_percent_format_impl(_py, &text, b) else {
                        return MoltObject::none().bits();
                    };
                    let out_ptr = alloc_string(_py, rendered.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
        }
        if !either_float && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let res = l_big.mod_floor(&r_big);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "float modulo");
            }
            let mut rem = lf % rf;
            if rem != 0.0 && (rem > 0.0) != (rf > 0.0) {
                rem += rf;
            }
            return float_result_bits(_py, rem);
        }
        unsafe {
            let mod_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.mod_name, b"__mod__");
            let rmod_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rmod_name, b"__rmod__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, mod_name_bits, rmod_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, err_op)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let imod_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.imod_name, b"__imod__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, imod_name_bits) {
                return res_bits;
            }
        }
        mod_impl(_py, a, b, "%=")
    })
}

fn complex_pow(base: ComplexParts, exp: ComplexParts) -> Result<ComplexParts, ()> {
    if base.re == 0.0 && base.im == 0.0 {
        if exp.re == 0.0 && exp.im == 0.0 {
            return Ok(ComplexParts { re: 1.0, im: 0.0 });
        }
        if exp.im != 0.0 || exp.re < 0.0 {
            return Err(());
        }
        return Ok(ComplexParts { re: 0.0, im: 0.0 });
    }
    let r = (base.re * base.re + base.im * base.im).sqrt();
    let theta = base.im.atan2(base.re);
    let log_r = r.ln();
    let u = exp.re * log_r - exp.im * theta;
    let v = exp.im * log_r + exp.re * theta;
    let exp_u = u.exp();
    Ok(ComplexParts {
        re: exp_u * v.cos(),
        im: exp_u * v.sin(),
    })
}

fn pow_i64_checked(base: i64, exp: i64) -> Option<i64> {
    if exp < 0 {
        return None;
    }
    let mut result: i128 = 1;
    let mut base_val: i128 = base as i128;
    let mut exp_val = exp as u64;
    let max = (1i128 << 46) - 1;
    let min = -(1i128 << 46);
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = result.saturating_mul(base_val);
            if result > max || result < min {
                return None;
            }
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base_val = base_val.saturating_mul(base_val);
            if base_val > max || base_val < min {
                return None;
            }
        }
    }
    Some(result as i64)
}

fn mod_py_i128(value: i128, modulus: i128) -> i128 {
    let mut rem = value % modulus;
    if rem != 0 && (rem > 0) != (modulus > 0) {
        rem += modulus;
    }
    rem
}

fn mod_pow_i128(_py: &PyToken<'_>, mut base: i128, exp: i64, modulus: i128) -> i128 {
    let mut result: i128 = 1;
    base = mod_py_i128(base, modulus);
    let mut exp_val = exp as u64;
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = mod_py_i128(result * base, modulus);
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base = mod_py_i128(base * base, modulus);
        }
    }
    mod_py_i128(result, modulus)
}

fn egcd_i128(a: i128, b: i128) -> (i128, i128, i128) {
    if b == 0 {
        return (a, 1, 0);
    }
    let (g, x, y) = egcd_i128(b, a % b);
    (g, y, x - (a / b) * y)
}

fn mod_inverse_i128(_py: &PyToken<'_>, value: i128, modulus: i128) -> Option<i128> {
    let (g, x, _) = egcd_i128(value, modulus);
    if g == 1 || g == -1 {
        Some(mod_py_i128(x, modulus))
    } else {
        None
    }
}

fn mod_pow_bigint(base: &BigInt, exp: u64, modulus: &BigInt) -> BigInt {
    let mut result = BigInt::from(1);
    let mut base_val = base.mod_floor(modulus);
    let mut exp_val = exp;
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = (result * &base_val).mod_floor(modulus);
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base_val = (&base_val * &base_val).mod_floor(modulus);
        }
    }
    result
}

fn egcd_bigint(a: BigInt, b: BigInt) -> (BigInt, BigInt, BigInt) {
    if b.is_zero() {
        return (a, BigInt::from(1), BigInt::from(0));
    }
    let (q, r) = a.div_mod_floor(&b);
    let (g, x, y) = egcd_bigint(b, r);
    (g, y.clone(), x - q * y)
}

fn mod_inverse_bigint(value: BigInt, modulus: &BigInt) -> Option<BigInt> {
    let (g, x, _) = egcd_bigint(value, modulus.clone());
    if g == BigInt::from(1) || g == BigInt::from(-1) {
        Some(x.mod_floor(modulus))
    } else {
        None
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pow(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { pow_impl(_py, a, b, "**") })
}

/// Power / `**` core. `err_op` selects the terminal TypeError symbol (`**` vs
/// `**=`); complex / numeric / dunder outcomes are spelling-independent.
fn pow_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(base)), Ok(Some(exp))) => {
                    return match complex_pow(base, exp) {
                        Ok(out) => complex_bits(_py, out.re, out.im),
                        Err(()) => raise_exception::<_>(
                            _py,
                            "ZeroDivisionError",
                            "zero to a negative or complex power",
                        ),
                    };
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            if ri >= 0 {
                if let Some(res) = pow_i64_checked(li, ri) {
                    return int_bits_from_i64(_py, res);
                }
                let res = BigInt::from(li).pow(ri as u32);
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
            let lf = li as f64;
            let rf = ri as f64;
            if lf == 0.0 && rf < 0.0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "0.0 cannot be raised to a negative power",
                );
            }
            let out = lf.powf(rf);
            if out.is_infinite() && lf.is_finite() && rf.is_finite() {
                return raise_exception::<_>(_py, "OverflowError", "math range error");
            }
            return float_result_bits(_py, out);
        }
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs))
        {
            if let Some(exp) = r_big.to_u64() {
                let res = l_big.pow(exp as u32);
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
            if r_big.is_negative()
                && let Some(lf) = l_big.to_f64()
            {
                let rf = r_big.to_f64().unwrap_or(f64::NEG_INFINITY);
                if lf == 0.0 && rf < 0.0 {
                    return raise_exception::<_>(
                        _py,
                        "ZeroDivisionError",
                        "0.0 cannot be raised to a negative power",
                    );
                }
                return float_result_bits(_py, lf.powf(rf));
            }
            return raise_exception::<_>(_py, "OverflowError", "exponent too large");
        }
        if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
            if lf == 0.0 && rf < 0.0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "0.0 cannot be raised to a negative power",
                );
            }
            if lf < 0.0 && rf.is_finite() && rf.fract() != 0.0 {
                let base = ComplexParts { re: lf, im: 0.0 };
                let exp = ComplexParts { re: rf, im: 0.0 };
                if let Ok(out) = complex_pow(base, exp) {
                    return complex_bits(_py, out.re, out.im);
                }
            }
            let out = lf.powf(rf);
            if out.is_infinite() && lf.is_finite() && rf.is_finite() {
                return raise_exception::<_>(_py, "OverflowError", "math range error");
            }
            return float_result_bits(_py, out);
        }
        unsafe {
            let pow_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.pow_name, b"__pow__");
            let rpow_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rpow_name, b"__rpow__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, pow_name_bits, rpow_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, err_op)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_pow(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let ipow_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ipow_name, b"__ipow__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ipow_name_bits) {
                return res_bits;
            }
        }
        pow_impl(_py, a, b, "**=")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pow_mod(a: u64, b: u64, m: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let mod_obj = obj_from_bits(m);
        // CPython rejects float arguments for 3-arg pow regardless of value.
        if lhs.is_float() || rhs.is_float() || mod_obj.is_float() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pow() 3rd argument not allowed unless all arguments are integers",
            );
        }
        if let (Some(li), Some(ri), Some(mi)) = (to_i64(lhs), to_i64(rhs), to_i64(mod_obj)) {
            let (base, exp, modulus) = (li as i128, ri, mi as i128);
            if modulus == 0 {
                return raise_exception::<_>(_py, "ValueError", "pow() 3rd argument cannot be 0");
            }
            let result = if exp < 0 {
                let mod_abs = modulus.abs();
                let base_mod = mod_py_i128(base, mod_abs);
                let Some(inv) = mod_inverse_i128(_py, base_mod, mod_abs) else {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "base is not invertible for the given modulus",
                    );
                };
                let inv_mod = mod_py_i128(inv, modulus);
                mod_pow_i128(_py, inv_mod, -exp, modulus)
            } else {
                mod_pow_i128(_py, base, exp, modulus)
            };
            return int_bits_from_i128(_py, result);
        }
        if let (Some(base), Some(exp), Some(modulus)) =
            (to_bigint(lhs), to_bigint(rhs), to_bigint(mod_obj))
        {
            if modulus.is_zero() {
                return raise_exception::<_>(_py, "ValueError", "pow() 3rd argument cannot be 0");
            }
            let result = if exp.is_negative() {
                let mod_abs = modulus.abs();
                let base_mod = base.mod_floor(&mod_abs);
                let Some(inv) = mod_inverse_bigint(base_mod, &mod_abs) else {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "base is not invertible for the given modulus",
                    );
                };
                let inv_mod = inv.mod_floor(&modulus);
                let neg_exp = -exp;
                if neg_exp.to_u64().is_none() {
                    return raise_exception::<_>(_py, "OverflowError", "exponent too large");
                }
                let exp_u64 = neg_exp.to_u64().unwrap();
                mod_pow_bigint(&inv_mod, exp_u64, &modulus)
            } else {
                if exp.to_u64().is_none() {
                    return raise_exception::<_>(_py, "OverflowError", "exponent too large");
                }
                let exp_u64 = exp.to_u64().unwrap();
                mod_pow_bigint(&base, exp_u64, &modulus)
            };
            if let Some(i) = bigint_to_inline(&result) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, result);
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            "pow() 3rd argument not allowed unless all arguments are integers",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_round(val_bits: u64, ndigits_bits: u64, has_ndigits_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let val = obj_from_bits(val_bits);
        let has_ndigits = to_i64(obj_from_bits(has_ndigits_bits)).unwrap_or(0) != 0;
        if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
            if !has_ndigits {
                return val_bits;
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                return val_bits;
            }
            let ndigits = index_i64_from_obj(_py, ndigits_bits, "round() ndigits must be int");
            if ndigits >= 0 {
                return val_bits;
            }
            let exp = (-ndigits) as u32;
            let value = unsafe { bigint_ref(ptr).clone() };
            let pow = BigInt::from(10).pow(exp);
            if pow.is_zero() {
                return val_bits;
            }
            let div = value.div_floor(&pow);
            let rem = value.mod_floor(&pow);
            let twice = &rem * 2;
            let mut rounded = div;
            if twice > pow || (twice == pow && !rounded.is_even()) {
                if value.is_negative() {
                    rounded -= 1;
                } else {
                    rounded += 1;
                }
            }
            let result = rounded * pow;
            if let Some(i) = bigint_to_inline(&result) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, result);
        }
        if !val.is_int()
            && !val.is_bool()
            && !val.is_float()
            && let Some(ptr) = maybe_ptr_from_bits(val_bits)
        {
            unsafe {
                let round_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.round_name, b"__round__");
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, round_name_bits) {
                    let ndigits_obj = obj_from_bits(ndigits_bits);
                    let want_arg = has_ndigits && !ndigits_obj.is_none();
                    let arity = callable_arity(_py, call_bits).unwrap_or(0);
                    let res_bits = if arity <= 1 {
                        if want_arg {
                            call_callable1(_py, call_bits, ndigits_bits)
                        } else {
                            call_callable0(_py, call_bits)
                        }
                    } else {
                        let arg_bits = if want_arg {
                            ndigits_bits
                        } else {
                            MoltObject::none().bits()
                        };
                        call_callable1(_py, call_bits, arg_bits)
                    };
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        if !val.is_float()
            && let Some(i) = to_i64(val)
        {
            if !has_ndigits {
                // Full-range boxing — `from_int` truncates a fit-i64 BigInt
                // (e.g. round(2**60)) or exact-integer float >= 2**46.
                return int_bits_from_i64(_py, i);
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                return int_bits_from_i64(_py, i);
            }
            let Some(ndigits) = to_i64(ndigits_obj) else {
                return raise_exception::<_>(_py, "TypeError", "round() ndigits must be int");
            };
            if ndigits >= 0 {
                return int_bits_from_i64(_py, i);
            }
            let exp = (-ndigits) as u32;
            if exp > 38 {
                return MoltObject::from_int(0).bits();
            }
            let pow = 10_i128.pow(exp);
            let value = i as i128;
            if pow == 0 {
                return int_bits_from_i64(_py, i);
            }
            let div = value / pow;
            let rem = value % pow;
            let abs_rem = rem.abs();
            let twice = abs_rem.saturating_mul(2);
            let mut rounded = div;
            if twice > pow || (twice == pow && (div & 1) != 0) {
                rounded += if value >= 0 { 1 } else { -1 };
            }
            let result = rounded.saturating_mul(pow);
            return int_bits_from_i128(_py, result);
        }
        if let Some(f) = to_f64(val) {
            if !has_ndigits {
                if f.is_nan() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cannot convert float NaN to integer",
                    );
                }
                if f.is_infinite() {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot convert float infinity to integer",
                    );
                }
                let rounded = round_half_even(f);
                let big = bigint_from_f64_trunc(rounded);
                if let Some(i) = bigint_to_inline(&big) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, big);
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                if f.is_nan() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cannot convert float NaN to integer",
                    );
                }
                if f.is_infinite() {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot convert float infinity to integer",
                    );
                }
                let rounded = round_half_even(f);
                let big = bigint_from_f64_trunc(rounded);
                if let Some(i) = bigint_to_inline(&big) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, big);
            }
            let Some(ndigits) = to_i64(ndigits_obj) else {
                return raise_exception::<_>(_py, "TypeError", "round() ndigits must be int");
            };
            let rounded = round_float_ndigits(f, ndigits);
            return float_result_bits(_py, rounded);
        }
        raise_exception::<_>(_py, "TypeError", "round() expects a real number")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trunc(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let val = obj_from_bits(val_bits);
        // A bare heap BigInt is already an exact int; return it unchanged
        // (retaining the owned alias) BEFORE the `to_i64` fast path so a
        // fit-i64 BigInt whose magnitude exceeds the 47-bit inline window is
        // not re-boxed through the truncating inline `from_int`.
        if bigint_ptr_from_bits(val_bits).is_some() {
            inc_ref_bits(_py, val_bits);
            return val_bits;
        }
        if let Some(i) = to_i64(val) {
            // Full-range boxing — never inline-only `from_int`, which would
            // silently truncate exact-integer floats or i64 magnitudes
            // >= 2**46 (mod 2**47).
            return int_bits_from_i64(_py, i);
        }
        if let Some(f) = to_f64(val) {
            if f.is_nan() {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "cannot convert float NaN to integer",
                );
            }
            if f.is_infinite() {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot convert float infinity to integer",
                );
            }
            let big = bigint_from_f64_trunc(f);
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, big);
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let trunc_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.trunc_name, b"__trunc__");
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, trunc_name_bits) {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        raise_exception::<_>(_py, "TypeError", "trunc() expects a real number")
    })
}

pub(super) fn set_like_result_type_id(type_id: u32) -> u32 {
    if type_id == TYPE_ID_FROZENSET {
        TYPE_ID_FROZENSET
    } else {
        TYPE_ID_SET
    }
}

pub(super) unsafe fn set_like_new_bits(type_id: u32, capacity: usize) -> u64 {
    if type_id == TYPE_ID_FROZENSET {
        molt_frozenset_new(capacity as u64)
    } else {
        molt_set_new(capacity as u64)
    }
}

pub(super) unsafe fn set_like_union(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len() + r_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
        }
        for &entry in r_elems.iter() {
            set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_intersection(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let l_hashes = set_hashes(lhs_ptr);
        let r_hashes = set_hashes(rhs_ptr);
        let (probe_elems, probe_hashes, probe_table, output) = if l_elems.len() <= r_elems.len() {
            (r_elems, r_hashes, set_table(rhs_ptr), l_elems)
        } else {
            (l_elems, l_hashes, set_table(lhs_ptr), r_elems)
        };
        let res_bits = set_like_new_bits(result_type_id, output.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in output.iter() {
            let found = set_find_entry(_py, probe_elems, probe_hashes, probe_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_some() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_difference(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let r_hashes = set_hashes(rhs_ptr);
        let r_table = set_table(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            let found = set_find_entry(_py, r_elems, r_hashes, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_symdiff(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let l_hashes = set_hashes(lhs_ptr);
        let r_hashes = set_hashes(rhs_ptr);
        let l_table = set_table(lhs_ptr);
        let r_table = set_table(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len() + r_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            let found = set_find_entry(_py, r_elems, r_hashes, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        for &entry in r_elems.iter() {
            let found = set_find_entry(_py, l_elems, l_hashes, l_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_copy_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let elems = set_order(ptr);
        let res_bits = set_like_new_bits(result_type_id, elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in elems.iter() {
            set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
        }
        res_bits
    }
}

/// Realize `other_bits` as a set-like pointer. When the argument is not already
/// a set/frozenset it is materialized into a temporary set, and `ctx` chooses
/// the unhashable-element error context for that materialization: probe-only
/// callers (`intersection`/`intersection_update`/`issubset`) pass
/// [`HashContext::Bare`]; all inserting callers pass [`HashContext::SetElement`].
pub(super) unsafe fn set_like_ptr_from_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
    ctx: HashContext,
) -> Option<(*mut u8, Option<u64>)> {
    unsafe {
        let obj = obj_from_bits(other_bits);
        if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET {
                return Some((ptr, None));
            }
        }
        let set_bits = set_from_iter_bits(_py, other_bits, ctx)?;
        let ptr = obj_from_bits(set_bits).as_ptr()?;
        Some((ptr, Some(set_bits)))
    }
}

/// Materialize an iterable into a fresh set. `ctx` selects the
/// unhashable-element error context (see [`set_like_ptr_from_bits`]).
pub(super) unsafe fn set_from_iter_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
    ctx: HashContext,
) -> Option<u64> {
    unsafe {
        let iter_bits = molt_iter(other_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, other_bits);
        }
        let set_bits = molt_set_new(0);
        let set_ptr = obj_from_bits(set_bits).as_ptr()?;
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let pair_ptr = pair_obj.as_ptr()?;
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            let pair_elems = seq_vec_ref(pair_ptr);
            if pair_elems.len() < 2 {
                return None;
            }
            let done_bits = pair_elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            let val_bits = pair_elems[0];
            set_add_in_place(_py, set_ptr, val_bits, ctx);
            if exception_pending(_py) {
                dec_ref_bits(_py, set_bits);
                return None;
            }
        }
        Some(set_bits)
    }
}

pub(super) fn binary_type_error(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: &str,
) -> u64 {
    // CPython uses "can only concatenate X (not 'Y') to X" for sequence +
    if op == "+" {
        let ltype = type_name(_py, lhs);
        let rtype = type_name(_py, rhs);
        if matches!(&*ltype, "str" | "list" | "tuple" | "bytes") && ltype != rtype {
            let msg = format!(
                "can only concatenate {} (not \"{}\") to {}",
                ltype, rtype, ltype
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    let msg = format!(
        "unsupported operand type(s) for {op}: '{}' and '{}'",
        type_name(_py, lhs),
        type_name(_py, rhs)
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn is_union_operand(_py: &PyToken<'_>, obj: MoltObject) -> bool {
    if obj.is_none() {
        return true;
    }
    let Some(ptr) = obj.as_ptr() else {
        return false;
    };
    unsafe {
        matches!(
            object_type_id(ptr),
            TYPE_ID_TYPE | TYPE_ID_GENERIC_ALIAS | TYPE_ID_UNION
        )
    }
}

fn append_union_arg(_py: &PyToken<'_>, args: &mut Vec<u64>, candidate: u64) {
    for &existing in args.iter() {
        if obj_eq(_py, obj_from_bits(existing), obj_from_bits(candidate)) {
            return;
        }
    }
    args.push(candidate);
}

fn collect_union_args(_py: &PyToken<'_>, bits: u64, args: &mut Vec<u64>) {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        append_union_arg(_py, args, builtin_classes(_py).none_type);
        return;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_UNION {
                let args_bits = union_type_args_bits(ptr);
                let args_obj = obj_from_bits(args_bits);
                if let Some(args_ptr) = args_obj.as_ptr()
                    && object_type_id(args_ptr) == TYPE_ID_TUPLE
                {
                    let elems = seq_vec_ref(args_ptr);
                    for &elem_bits in elems.iter() {
                        append_union_arg(_py, args, elem_bits);
                    }
                    return;
                }
                append_union_arg(_py, args, args_bits);
                return;
            }
        }
    }
    append_union_arg(_py, args, bits);
}

fn build_union_type(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> u64 {
    let mut args = Vec::new();
    collect_union_args(_py, lhs.bits(), &mut args);
    collect_union_args(_py, rhs.bits(), &mut args);
    if args.len() == 1 {
        let bits = args[0];
        inc_ref_bits(_py, bits);
        return bits;
    }
    let tuple_ptr = alloc_tuple(_py, args.as_slice());
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
    let union_ptr = alloc_union_type(_py, args_bits);
    if union_ptr.is_null() {
        dec_ref_bits(_py, args_bits);
        return MoltObject::none().bits();
    }
    dec_ref_bits(_py, args_bits);
    MoltObject::from_ptr(union_ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bit_or(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            if lhs.is_bool() && rhs.is_bool() {
                return MoltObject::from_bool((li != 0) | (ri != 0)).bits();
            }
            let res = li | ri;
            if inline_int_from_i128(res as i128).is_some() {
                return int_bits_from_i64(_py, res);
            }
            return bigint_bits(_py, BigInt::from(li) | BigInt::from(ri));
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big | r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if is_union_operand(_py, lhs) && is_union_operand(_py, rhs) {
            return build_union_type(_py, lhs, rhs);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_union(_py, lp, rp, set_like_result_type_id(ltype));
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let (rhs_ptr, rhs_bits) = if is_set_like_type(rtype) {
                        (rp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, rp, rtype) else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_union(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
                if ltype == TYPE_ID_DICT && rtype == TYPE_ID_DICT {
                    let builtins = builtin_classes(_py);
                    let lhs_class = object_class_bits(lp);
                    let rhs_class = object_class_bits(rp);
                    let lhs_exact = lhs_class == 0 || lhs_class == builtins.dict;
                    let rhs_exact = rhs_class == 0 || rhs_class == builtins.dict;
                    if !lhs_exact || !rhs_exact {
                        // Dict subclasses must dispatch through dunder resolution.
                        // Skip the dict fast-path so __or__/__ror__ can run.
                        // (Exact dict stays on the optimized union path.)
                    } else if let (Some(lhs_bits), Some(rhs_bits)) = (
                        dict_like_bits_from_ptr(_py, lp),
                        dict_like_bits_from_ptr(_py, rp),
                    ) {
                        let out_bits = molt_dict_copy(lhs_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        let _ = molt_dict_update(out_bits, rhs_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, out_bits);
                            return MoltObject::none().bits();
                        }
                        return out_bits;
                    }
                }
            }
        }
        unsafe {
            let or_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.or_name, b"__or__");
            let ror_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ror_name, b"__ror__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, or_name_bits, ror_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "|")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_bit_or(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "|=", a, b);
                    }
                    let _ = molt_set_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    let builtins = builtin_classes(_py);
                    let class_bits = object_class_bits(ptr);
                    let exact_dict = class_bits == 0 || class_bits == builtins.dict;
                    if exact_dict {
                        if let Some(rhs_ptr) = obj_from_bits(b).as_ptr()
                            && dict_like_bits_from_ptr(_py, rhs_ptr).is_some()
                        {
                            let _ = molt_dict_update(a, b);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            inc_ref_bits(_py, a);
                            return a;
                        }
                        return raise_unsupported_inplace(_py, "|=", a, b);
                    }
                }
            }
        }
        unsafe {
            let ior_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ior_name, b"__ior__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ior_name_bits) {
                return res_bits;
            }
        }
        molt_bit_or(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bit_and(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            if lhs.is_bool() && rhs.is_bool() {
                return MoltObject::from_bool((li != 0) & (ri != 0)).bits();
            }
            let res = li & ri;
            if inline_int_from_i128(res as i128).is_some() {
                return int_bits_from_i64(_py, res);
            }
            return bigint_bits(_py, BigInt::from(li) & BigInt::from(ri));
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big & r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_intersection(_py, lp, rp, set_like_result_type_id(ltype));
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let (rhs_ptr, rhs_bits) = if is_set_like_type(rtype) {
                        (rp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, rp, rtype) else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_intersection(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
            }
        }
        unsafe {
            let and_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.and_name, b"__and__");
            let rand_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rand_name, b"__rand__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, and_name_bits, rand_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "&")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_bit_and(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "&=", a, b);
                    }
                    let _ = molt_set_intersection_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        unsafe {
            let iand_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.iand_name, b"__iand__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, iand_name_bits) {
                return res_bits;
            }
        }
        molt_bit_and(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bit_xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 ** 3.0 must return 8.0, not 8).
        if !lhs.is_float()
            && !rhs.is_float()
            && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs))
        {
            if lhs.is_bool() && rhs.is_bool() {
                return MoltObject::from_bool((li != 0) ^ (ri != 0)).bits();
            }
            let res = li ^ ri;
            if inline_int_from_i128(res as i128).is_some() {
                return int_bits_from_i64(_py, res);
            }
            return bigint_bits(_py, BigInt::from(li) ^ BigInt::from(ri));
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big ^ r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_symdiff(_py, lp, rp, set_like_result_type_id(ltype));
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let (rhs_ptr, rhs_bits) = if is_set_like_type(rtype) {
                        (rp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, rp, rtype) else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_symdiff(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
            }
        }
        unsafe {
            let xor_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.xor_name, b"__xor__");
            let rxor_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rxor_name, b"__rxor__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, xor_name_bits, rxor_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "^")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_invert(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        // Python 3.12+ DeprecationWarning for ~bool.
        // Constant bools (~True/~False) are handled at compile time by
        // _prescan_compile_warnings. Variable-typed ~x is caught here
        // at runtime.
        if obj.is_bool() {
            let msg = concat!(
                "Bitwise inversion '~' on bool is deprecated and will be ",
                "removed in Python 3.16. This returns the bitwise inversion ",
                "of the underlying int object and is usually not what you ",
                "expect from negating a bool. Use the 'not' operator for ",
                "boolean negation or ~int(x) if you really want the bitwise ",
                "inversion of the underlying int."
            );
            crate::builtins::warnings_ext::emit_deprecation_warning(_py, msg);
        }
        if let Some(i) = to_i64(obj) {
            let res = -(i as i128) - 1;
            return int_bits_from_i128(_py, res);
        }
        if let Some(big) = to_bigint(obj) {
            let res = -big - BigInt::from(1);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some(ptr) = maybe_ptr_from_bits(val)
            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__invert__")
        {
            unsafe {
                let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        let msg = format!("bad operand type for unary ~: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_neg(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            let res = -(i as i128);
            return int_bits_from_i128(_py, res);
        }
        if let Some(big) = to_bigint(obj) {
            let res = -big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some(f) = to_f64(obj) {
            return float_result_bits(_py, -f);
        }
        if let Some(ptr) = complex_ptr_from_bits(val) {
            let value = unsafe { *complex_ref(ptr) };
            return complex_bits(_py, -value.re, -value.im);
        }
        if let Some(ptr) = maybe_ptr_from_bits(val)
            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__neg__")
        {
            unsafe {
                let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("bad operand type for unary -: '{type_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pos(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            // Full-range boxing — `from_int` would silently truncate a fit-i64
            // BigInt or exact-integer float with magnitude >= 2**46.
            return int_bits_from_i64(_py, i);
        }
        if let Some(big) = to_bigint(obj) {
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, big);
        }
        if let Some(f) = to_f64(obj) {
            return float_result_bits(_py, f);
        }
        if let Some(ptr) = complex_ptr_from_bits(val) {
            let value = unsafe { *complex_ref(ptr) };
            return complex_bits(_py, value.re, value.im);
        }
        if let Some(ptr) = maybe_ptr_from_bits(val)
            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__pos__")
        {
            unsafe {
                let call_bits = attr_lookup_ptr_allow_missing(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("bad operand type for unary +: '{type_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_bit_xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "^=", a, b);
                    }
                    let _ = molt_set_symdiff_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        unsafe {
            let ixor_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ixor_name, b"__ixor__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ixor_name_bits) {
                return res_bits;
            }
        }
        molt_bit_xor(a, b)
    })
}

#[inline]
fn trace_bigint_shift_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_BIGINT_SHIFT").as_deref() == Ok("1"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShiftCount {
    FitsUsize(usize),
    TooLarge,
}

enum StrictInteger {
    Inline(i64),
    Big(BigInt),
}

impl StrictInteger {
    fn is_zero(&self) -> bool {
        match self {
            Self::Inline(value) => *value == 0,
            Self::Big(value) => value.is_zero(),
        }
    }

    fn magnitude_bits(&self) -> u64 {
        match self {
            Self::Inline(value) => BigInt::from(*value).bits(),
            Self::Big(value) => value.bits(),
        }
    }

    fn is_negative(&self) -> bool {
        match self {
            Self::Inline(value) => *value < 0,
            Self::Big(value) => value.is_negative(),
        }
    }
}

fn strict_integer_from_obj(obj_bits: u64) -> Option<StrictInteger> {
    let obj = obj_from_bits(obj_bits);
    if obj.is_int() {
        return Some(StrictInteger::Inline(obj.as_int_unchecked()));
    }
    if obj.is_bool() {
        return Some(StrictInteger::Inline(if (obj.bits() & 0x1) == 1 {
            1
        } else {
            0
        }));
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj_bits) {
        return Some(StrictInteger::Big(unsafe { bigint_ref(ptr).clone() }));
    }
    if let Some(bits) = int_subclass_value_bits_raw(obj_bits) {
        let val_obj = obj_from_bits(bits);
        if val_obj.is_int() {
            return Some(StrictInteger::Inline(val_obj.as_int_unchecked()));
        }
        if val_obj.is_bool() {
            return Some(StrictInteger::Inline(if (val_obj.bits() & 0x1) == 1 {
                1
            } else {
                0
            }));
        }
        if let Some(ptr) = bigint_ptr_from_bits(bits) {
            return Some(StrictInteger::Big(unsafe { bigint_ref(ptr).clone() }));
        }
    }
    None
}

fn shift_count_from_integer(_py: &PyToken<'_>, count: StrictInteger) -> Option<ShiftCount> {
    match count {
        StrictInteger::Inline(value) => {
            if value < 0 {
                raise_exception::<u64>(_py, "ValueError", "negative shift count");
                return None;
            }
            Some(match usize::try_from(value) {
                Ok(value) => ShiftCount::FitsUsize(value),
                Err(_) => ShiftCount::TooLarge,
            })
        }
        StrictInteger::Big(value) => {
            if value.is_negative() {
                raise_exception::<u64>(_py, "ValueError", "negative shift count");
                return None;
            }
            Some(match value.to_usize() {
                Some(value) => ShiftCount::FitsUsize(value),
                None => ShiftCount::TooLarge,
            })
        }
    }
}

fn right_shift_saturation_result(_py: &PyToken<'_>, value: &StrictInteger) -> u64 {
    if value.is_negative() {
        int_bits_from_i64(_py, -1)
    } else {
        int_bits_from_i64(_py, 0)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { lshift_impl(_py, a, b, "<<") })
}

/// Left-shift core. `err_op` selects the terminal TypeError symbol (`<<` vs
/// `<<=`); numeric / overflow outcomes are spelling-independent.
fn lshift_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let trace_shift = trace_bigint_shift_enabled();
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // CPython tries `a.__lshift__(b)` then `b.__rlshift__(a)` for any operand
        // the integer fast path can't consume (e.g. a user class defining
        // `__lshift__`). Only fall to TypeError once that chain is exhausted —
        // raising on the first non-integer operand skipped the dunder protocol
        // entirely (so `x << y` on a custom class wrongly raised).
        let (Some(value), Some(count)) = (strict_integer_from_obj(a), strict_integer_from_obj(b))
        else {
            unsafe {
                let lshift_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.lshift_name,
                    b"__lshift__",
                );
                let rlshift_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.rlshift_name,
                    b"__rlshift__",
                );
                if let Some(res_bits) =
                    call_binary_dunder(_py, a, b, lshift_name_bits, rlshift_name_bits)
                {
                    return res_bits;
                }
            }
            return binary_type_error(_py, lhs, rhs, err_op);
        };
        let Some(shift) = shift_count_from_integer(_py, count) else {
            return MoltObject::none().bits();
        };
        if value.is_zero() {
            return int_bits_from_i64(_py, 0);
        }
        let shift_u = match shift {
            ShiftCount::FitsUsize(value) => value,
            ShiftCount::TooLarge => {
                return raise_exception::<_>(_py, "OverflowError", "too many digits in integer");
            }
        };
        let value_bits = value.magnitude_bits();
        let shift_bits = match u64::try_from(shift_u) {
            Ok(value) => value,
            Err(_) => {
                return raise_exception::<_>(_py, "OverflowError", "too many digits in integer");
            }
        };
        if let Err(msg) = crate::resource::check_lshift_size(value_bits, shift_bits) {
            return raise_exception::<_>(_py, "MemoryError", &msg);
        }
        let value = match value {
            StrictInteger::Inline(value) => {
                if shift_u < 127
                    && let Some(result) = (value as i128).checked_shl(shift_u as u32)
                {
                    return int_bits_from_i128(_py, result);
                }
                BigInt::from(value)
            }
            StrictInteger::Big(value) => value,
        };
        let value_for_trace = trace_shift.then(|| value.clone());
        let res = value << shift_u;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        let bits = bigint_bits(_py, res.clone());
        if trace_shift {
            eprintln!(
                "[molt shift] bigint lhs={} shift={} branch=bigint result={} bits=0x{:x}",
                value_for_trace.expect("trace clone present"),
                shift_u,
                res,
                bits
            );
        }
        bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let ilshift_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.ilshift_name,
                b"__ilshift__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ilshift_name_bits) {
                return res_bits;
            }
        }
        lshift_impl(_py, a, b, "<<=")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { rshift_impl(_py, a, b, ">>") })
}

/// Right-shift core. `err_op` selects the terminal TypeError symbol (`>>` vs
/// `>>=`); numeric / saturation outcomes are spelling-independent.
fn rshift_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Try `a.__rshift__(b)` then `b.__rrshift__(a)` for any operand the
        // integer fast path can't consume (mirrors `lshift_impl`).
        let (Some(value), Some(count)) = (strict_integer_from_obj(a), strict_integer_from_obj(b))
        else {
            unsafe {
                let rshift_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.rshift_name,
                    b"__rshift__",
                );
                let rrshift_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.rrshift_name,
                    b"__rrshift__",
                );
                if let Some(res_bits) =
                    call_binary_dunder(_py, a, b, rshift_name_bits, rrshift_name_bits)
                {
                    return res_bits;
                }
            }
            return binary_type_error(_py, lhs, rhs, err_op);
        };
        let Some(shift) = shift_count_from_integer(_py, count) else {
            return MoltObject::none().bits();
        };
        if value.is_zero() {
            return int_bits_from_i64(_py, 0);
        }
        let shift_u = match shift {
            ShiftCount::FitsUsize(value) => value,
            ShiftCount::TooLarge => return right_shift_saturation_result(_py, &value),
        };
        if u64::try_from(shift_u).map_or(true, |shift_bits| shift_bits >= value.magnitude_bits()) {
            return right_shift_saturation_result(_py, &value);
        }
        let res = match value {
            StrictInteger::Inline(value) => return int_bits_from_i64(_py, value >> shift_u),
            StrictInteger::Big(value) => value >> shift_u,
        };
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        bigint_bits(_py, res)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let irshift_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.irshift_name,
                b"__irshift__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, irshift_name_bits) {
                return res_bits;
            }
        }
        rshift_impl(_py, a, b, ">>=")
    })
}

#[cfg(test)]
mod tests {
    use crate::builtins::exceptions::molt_exception_message;
    use crate::*;
    use num_bigint::BigInt;

    fn repr_string(_py: &PyToken<'_>, bits: u64) -> String {
        let repr_bits = crate::molt_repr_from_obj(bits);
        let rendered =
            string_obj_to_owned(obj_from_bits(repr_bits)).expect("repr must be a string object");
        dec_ref_bits(_py, repr_bits);
        rendered
    }

    fn pending_exception_kind_and_message(_py: &PyToken<'_>) -> (String, String) {
        let exc_bits = crate::molt_exception_last();
        let kind_bits = crate::molt_exception_kind(exc_bits);
        let msg_bits = molt_exception_message(exc_bits);
        let kind =
            string_obj_to_owned(obj_from_bits(kind_bits)).expect("exception kind must be string");
        let msg =
            string_obj_to_owned(obj_from_bits(msg_bits)).expect("exception message must be string");
        dec_ref_bits(_py, msg_bits);
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, exc_bits);
        let _ = crate::molt_exception_clear();
        (kind, msg)
    }

    #[test]
    fn molt_lshift_promotes_bigint_operand_correctly() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Clear any pending exception leaked from prior tests so that
        // `molt_repr_from_obj`'s pending-exception fast path doesn't return
        // None and trip the assertion below.
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let lhs = int_bits_from_i128(_py, 283686952306183);
            let rhs = MoltObject::from_int(8).bits();
            let out = molt_lshift(lhs, rhs);
            let repr_bits = crate::molt_repr_from_obj(out);
            let rendered = string_obj_to_owned(obj_from_bits(repr_bits))
                .expect("repr must be a string object");
            dec_ref_bits(_py, repr_bits);
            dec_ref_bits(_py, out);
            assert_eq!(rendered, "72623859790382848");
        });
    }

    #[test]
    fn molt_shift_counts_do_not_truncate_at_u32_width() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let huge_inline_count = int_bits_from_i128(_py, 1_i128 << 32);
            let one = MoltObject::from_int(1).bits();
            let minus_eight = MoltObject::from_int(-8).bits();

            let pos_out = molt_rshift(one, huge_inline_count);
            assert_eq!(repr_string(_py, pos_out), "0");
            assert_eq!(crate::molt_exception_pending(), 0);

            let neg_out = molt_rshift(minus_eight, huge_inline_count);
            assert_eq!(repr_string(_py, neg_out), "-1");
            assert_eq!(crate::molt_exception_pending(), 0);
        });
    }

    #[test]
    fn molt_shift_counts_accept_heap_bigint_sign_without_i64_narrowing() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let huge_count = int_bits_from_i128(_py, 1_i128 << 70);
            let huge_negative_count = int_bits_from_i128(_py, -(1_i128 << 70));

            let zero_lshift = molt_lshift(MoltObject::from_int(0).bits(), huge_count);
            assert_eq!(repr_string(_py, zero_lshift), "0");
            assert_eq!(crate::molt_exception_pending(), 0);

            let right = molt_rshift(MoltObject::from_int(1).bits(), huge_count);
            assert_eq!(repr_string(_py, right), "0");
            assert_eq!(crate::molt_exception_pending(), 0);

            let invalid = molt_lshift(MoltObject::from_int(1).bits(), huge_negative_count);
            assert!(obj_from_bits(invalid).is_none());
            let (kind, msg) = pending_exception_kind_and_message(_py);
            assert_eq!(kind, "ValueError");
            assert_eq!(msg, "negative shift count");
        });
    }

    #[test]
    fn molt_lshift_rejects_unallocatable_counts_before_bigint_shift() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let count = bigint_bits(_py, BigInt::from(10u32).pow(100));
            let out = molt_lshift(MoltObject::from_int(1).bits(), count);
            assert!(obj_from_bits(out).is_none());
            let (kind, msg) = pending_exception_kind_and_message(_py);
            assert_eq!(kind, "OverflowError");
            assert_eq!(msg, "too many digits in integer");

            let resource_limited_count = int_bits_from_i128(_py, 1_i128 << 32);
            let out = molt_lshift(MoltObject::from_int(1).bits(), resource_limited_count);
            assert!(obj_from_bits(out).is_none());
            let (kind, msg) = pending_exception_kind_and_message(_py);
            assert_eq!(kind, "MemoryError");
            assert!(msg.starts_with("left shift result too large:"));
        });
    }

    #[test]
    fn molt_shift_operands_reject_exact_floats_and_generic_index_objects() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let one = MoltObject::from_int(1).bits();
            let one_float = MoltObject::from_float(1.0).bits();
            let out = molt_lshift(one, one_float);
            assert!(obj_from_bits(out).is_none());
            let (kind, msg) = pending_exception_kind_and_message(_py);
            assert_eq!(kind, "TypeError");
            assert_eq!(msg, "unsupported operand type(s) for <<: 'int' and 'float'");

            let out = molt_lshift(one_float, MoltObject::from_int(2).bits());
            assert!(obj_from_bits(out).is_none());
            let (kind, msg) = pending_exception_kind_and_message(_py);
            assert_eq!(kind, "TypeError");
            assert_eq!(msg, "unsupported operand type(s) for <<: 'float' and 'int'");
        });
    }

    /// `bigint_true_divide` must produce the exact same IEEE-754 double as
    /// CPython's `long_true_divide` (`int / int`), bit-for-bit. Reference bit
    /// patterns were captured from CPython 3.12 via
    /// `struct.pack('<d', a / b)`.
    #[test]
    fn bigint_true_divide_matches_cpython_bit_exact() {
        // (numerator, denominator, expected f64 bits from CPython `a / b`).
        let cases: &[(&str, &str, u64)] = &[
            ("1152921504606846976", "2", 0x43a0_0000_0000_0000), // (1<<60)/2
            ("1152921504606846976", "3", 0x4395_5555_5555_5555), // (1<<60)/3
            ("1152921504606846977", "2", 0x43a0_0000_0000_0000), // ((1<<60)+1)/2 (ties-to-even)
            (
                "1000000000000000000000000000000",
                "7",
                0x45fc_d98a_8b00_a10b,
            ), // 10**30/7
            ("-1180591620717411303424", "3", 0xc435_5555_5555_5555), // -(1<<70)/3
            ("1", "3", 0x3fd5_5555_5555_5555),
            (
                "1267650600228229401496703205376", // 2**100
                "1125899906842624",                // 2**50
                0x4310_0000_0000_0000,
            ),
            ("1152921504606846976", "1", 0x43b0_0000_0000_0000), // (1<<60)/1
            (
                "-10000000000000000000000000000000000000000", // -(10**40)
                "99991",
                0xc733_42d3_0df6_e471,
            ),
            ("0", "5", 0x0000_0000_0000_0000), // 0/5 == +0.0
            // 2**1000 / 3: large numerator near the top of the f64 range.
            (
                "10715086071862673209484250490600018105614048117055336074437503883703510511249361224931983788156958581275946729175531468251871452856923140435984577574698574803934567774824230985421074605062371141877954182153046474983581941267398767559165543946077062914571196477686542167660429831652624386837205668069376",
                "3",
                0x7e55_5555_5555_5555,
            ),
            // 3 / 2**1000: tiny quotient, exercises the subnormal-adjacent path.
            (
                "3",
                "10715086071862673209484250490600018105614048117055336074437503883703510511249361224931983788156958581275946729175531468251871452856923140435984577574698574803934567774824230985421074605062371141877954182153046474983581941267398767559165543946077062914571196477686542167660429831652624386837205668069376",
                0x0188_0000_0000_0000,
            ),
        ];
        for (num_s, den_s, expected_bits) in cases {
            let num: BigInt = num_s.parse().expect("numerator parses");
            let den: BigInt = den_s.parse().expect("denominator parses");
            let got = super::bigint_true_divide(&num, &den)
                .unwrap_or_else(|| panic!("{num_s} / {den_s} unexpectedly overflowed f64"));
            assert_eq!(
                got.to_bits(),
                *expected_bits,
                "{num_s} / {den_s}: got {got:?} (0x{:016x}), expected 0x{expected_bits:016x}",
                got.to_bits()
            );
        }
    }

    /// Magnitudes beyond `f64::MAX` must report overflow (mapped to
    /// `OverflowError` by the caller), matching CPython.
    #[test]
    fn bigint_true_divide_overflows_past_f64_max() {
        // 2**2000 / 1 far exceeds f64::MAX (~1.8e308).
        let num: BigInt = BigInt::from(2).pow(2000);
        let den: BigInt = BigInt::from(1);
        assert!(super::bigint_true_divide(&num, &den).is_none());
        // Negative direction overflows too.
        assert!(super::bigint_true_divide(&(-num.clone()), &den).is_none());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_matmul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { matmul_impl(_py, a, b, "@") })
}

/// Matrix-multiply core. `err_op` selects the terminal TypeError symbol (`@`
/// vs `@=`); the buffer2d fast path and dunder outcomes are spelling-
/// independent.
fn matmul_impl(_py: &PyToken<'_>, a: u64, b: u64, err_op: &str) -> u64 {
    {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                if object_type_id(lp) == TYPE_ID_BUFFER2D && object_type_id(rp) == TYPE_ID_BUFFER2D
                {
                    return molt_buffer2d_matmul(a, b);
                }
            }
        }
        unsafe {
            let matmul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.matmul_name, b"__matmul__");
            let rmatmul_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.rmatmul_name,
                b"__rmatmul__",
            );
            if let Some(res_bits) =
                call_binary_dunder(_py, a, b, matmul_name_bits, rmatmul_name_bits)
            {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, err_op)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_matmul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let imatmul_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.imatmul_name,
                b"__imatmul__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, imatmul_name_bits) {
                return res_bits;
            }
        }
        matmul_impl(_py, a, b, "@=")
    })
}
