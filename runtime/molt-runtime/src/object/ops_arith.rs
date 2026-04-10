// Arithmetic, bitwise, and percent-format operations.
// Split from ops.rs for compilation-unit size reduction.

use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};

use super::ops::{
    as_float_extended, call_binary_dunder, call_inplace_dunder, concat_bytes_like,
    fill_repeated_bytes, float_result_bits, is_float_extended,
};

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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        // Note: exception_pending check removed — backends guarantee molt_add
        // is only called on non-exception paths, so the TLS + atomic overhead
        // of checking every arithmetic op is unnecessary.
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                let ltype = object_type_id(ptr);
                if ltype == TYPE_ID_LIST {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
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
    crate::with_gil_entry!(_py, {
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
                TYPE_ID_LIST => alloc_list(_py, &[]),
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
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(ptr);
                let total = match elems.len().checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
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
        let elems = seq_vec(ptr);
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
        elems.reserve(total.saturating_sub(snapshot.len()));
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
        let elems = bytearray_vec(ptr);
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
        elems.reserve(total.saturating_sub(snapshot.len()));
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
        bytearray_vec(ptr).extend_from_slice(&payload);
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                if ltype == TYPE_ID_LIST || ltype == TYPE_ID_BYTEARRAY {
                    let rhs_type = type_name(_py, obj_from_bits(b));
                    let msg = format!("can't multiply sequence by non-int of type '{rhs_type}'");
                    let count = index_i64_from_obj(_py, b, &msg);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let ok = if ltype == TYPE_ID_LIST {
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
        molt_mul(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        if bigint_ptr_from_bits(a).is_some() || bigint_ptr_from_bits(b).is_some() {
            return raise_exception::<_>(_py, "OverflowError", "int too large to convert to float");
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
        binary_type_error(_py, lhs, rhs, "/")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_div(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        binary_type_error(_py, lhs, rhs, "//")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_floordiv(a, b)
    })
}

#[derive(Clone, Copy, Default)]
struct PercentFormatFlags {
    left_adjust: bool,
    sign_plus: bool,
    sign_space: bool,
    zero_pad: bool,
    alternate: bool,
}

fn percent_object_has_getitem(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
        return false;
    };
    let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
    dec_ref_bits(_py, name_bits);
    if let Some(call_bits) = call_bits {
        dec_ref_bits(_py, call_bits);
        return true;
    }
    false
}

fn percent_rhs_allows_unused_non_tuple(_py: &PyToken<'_>, rhs: MoltObject) -> bool {
    let Some(ptr) = rhs.as_ptr() else {
        return false;
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_STRING || type_id == TYPE_ID_TUPLE {
            return false;
        }
    }
    percent_object_has_getitem(_py, ptr)
}

fn percent_parse_usize(
    _py: &PyToken<'_>,
    bytes: &[u8],
    idx: &mut usize,
    field_name: &str,
) -> Option<usize> {
    let start = *idx;
    let mut out: usize = 0;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        let digit = (bytes[*idx] - b'0') as usize;
        out = match out.checked_mul(10).and_then(|v| v.checked_add(digit)) {
            Some(v) => v,
            None => {
                let msg = format!("{field_name} too large in format string");
                return raise_exception::<Option<usize>>(_py, "ValueError", &msg);
            }
        };
        *idx += 1;
    }
    if *idx == start { None } else { Some(out) }
}

fn percent_unsupported_char(_py: &PyToken<'_>, ch: u8, idx: usize) -> Option<String> {
    let ch_display = ch as char;
    let msg = format!("unsupported format character '{ch_display}' (0x{ch:02x}) at index {idx}");
    raise_exception::<Option<String>>(_py, "ValueError", &msg)
}

fn percent_apply_width(
    text: String,
    width: Option<usize>,
    left_adjust: bool,
    pad_char: char,
) -> String {
    let Some(width) = width else {
        return text;
    };
    let text_len = text.chars().count();
    if text_len >= width {
        return text;
    }
    let pad_len = width - text_len;
    let padding = pad_char.to_string().repeat(pad_len);
    if left_adjust {
        format!("{text}{padding}")
    } else {
        format!("{padding}{text}")
    }
}

fn percent_apply_numeric_width(
    prefix: &str,
    body: String,
    width: Option<usize>,
    left_adjust: bool,
    zero_pad: bool,
) -> String {
    let prefix_len = prefix.chars().count();
    let body_len = body.chars().count();
    if zero_pad
        && !left_adjust
        && let Some(width) = width
        && width > prefix_len + body_len
    {
        let mut out = String::with_capacity(width);
        out.push_str(prefix);
        out.push_str(&"0".repeat(width - prefix_len - body_len));
        out.push_str(&body);
        return out;
    }
    let mut text = String::with_capacity(prefix.len() + body.len());
    text.push_str(prefix);
    text.push_str(&body);
    percent_apply_width(text, width, left_adjust, ' ')
}

fn percent_raise_real_type_error_decimal(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
) -> Option<BigInt> {
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: a real number is required, not {}",
        type_name(_py, obj)
    );
    raise_exception::<Option<BigInt>>(_py, "TypeError", &msg)
}

fn percent_raise_integer_type_error(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
) -> Option<BigInt> {
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: an integer is required, not {}",
        type_name(_py, obj)
    );
    raise_exception::<Option<BigInt>>(_py, "TypeError", &msg)
}

fn percent_raise_real_type_error_f(_py: &PyToken<'_>, obj: MoltObject) -> Option<f64> {
    let msg = format!("must be real number, not {}", type_name(_py, obj));
    raise_exception::<Option<f64>>(_py, "TypeError", &msg)
}

fn percent_raise_char_type_error(_py: &PyToken<'_>, obj: MoltObject) -> Option<char> {
    let _ = obj;
    raise_exception::<Option<char>>(_py, "TypeError", "%c requires int or char")
}

fn percent_char_from_bigint(_py: &PyToken<'_>, value: BigInt) -> Option<char> {
    let max_code = BigInt::from(0x110000u32);
    if value.sign() == Sign::Minus || value >= max_code {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    }
    let Some(code) = value.to_u32() else {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    };
    let Some(ch) = char::from_u32(code) else {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    };
    Some(ch)
}

fn percent_decimal_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return Some(unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(f) = as_float_extended(obj) {
        if f.is_nan() {
            return raise_exception::<Option<BigInt>>(
                _py,
                "ValueError",
                "cannot convert float NaN to integer",
            );
        }
        if f.is_infinite() {
            return raise_exception::<Option<BigInt>>(
                _py,
                "OverflowError",
                "cannot convert float infinity to integer",
            );
        }
        return Some(bigint_from_f64_trunc(f));
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_real_type_error_decimal(_py, obj, conv);
            }
            let int_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.int_name, b"__int__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, int_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__int__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_real_type_error_decimal(_py, obj, conv)
}

fn percent_integer_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return Some(unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_integer_type_error(_py, obj, conv);
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_integer_type_error(_py, obj, conv)
}

fn percent_char_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<char> {
    let obj = obj_from_bits(value_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        let mut chars = text.chars();
        return match chars.next() {
            Some(ch) if chars.next().is_none() => Some(ch),
            _ => percent_raise_char_type_error(_py, obj),
        };
    }
    if let Some(i) = to_i64(obj) {
        return percent_char_from_bigint(_py, BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return percent_char_from_bigint(_py, unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return percent_char_from_bigint(_py, BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return percent_char_from_bigint(_py, out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<char>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_char_type_error(_py, obj)
}

fn percent_float_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<f64> {
    let obj = obj_from_bits(value_bits);
    if let Some(f) = as_float_extended(obj) {
        return Some(f);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return match unsafe { bigint_ref(big_ptr) }.to_f64() {
            Some(v) => Some(v),
            None => raise_exception::<Option<f64>>(
                _py,
                "OverflowError",
                "int too large to convert to float",
            ),
        };
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_real_type_error_f(_py, obj);
            }
            let float_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(f) = res_obj.as_float() {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(f);
                }
                let owner = class_name_for_error(type_of_bits(_py, value_bits));
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(i as f64);
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).to_f64();
                    dec_ref_bits(_py, res_bits);
                    return match out {
                        Some(v) => Some(v),
                        None => raise_exception::<Option<f64>>(
                            _py,
                            "OverflowError",
                            "int too large to convert to float",
                        ),
                    };
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_real_type_error_f(_py, obj)
}

fn percent_numeric_prefix(is_negative: bool, flags: PercentFormatFlags) -> Option<char> {
    if is_negative {
        Some('-')
    } else if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    }
}

fn percent_format_text(
    text: String,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> String {
    let rendered = if let Some(precision) = precision {
        text.chars().take(precision).collect::<String>()
    } else {
        text
    };
    percent_apply_width(rendered, width, flags.left_adjust, ' ')
}

fn percent_format_decimal(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_decimal_from_obj(_py, value_bits, conv)?;
    let negative = value.is_negative();
    let mut body = value.abs().to_string();
    if let Some(precision) = precision
        && body.len() < precision
    {
        body = format!("{}{}", "0".repeat(precision - body.len()), body);
    }
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    let zero_pad = flags.zero_pad && !flags.left_adjust;
    Some(percent_apply_numeric_width(
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        zero_pad,
    ))
}

fn percent_format_radix(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_integer_from_obj(_py, value_bits, conv)?;
    let negative = value.is_negative();
    let mut body = match conv {
        b'o' => value.abs().to_str_radix(8),
        b'x' | b'X' => value.abs().to_str_radix(16),
        _ => value.abs().to_string(),
    };
    if conv == b'X' {
        body = body.to_uppercase();
    }
    if let Some(precision) = precision
        && body.len() < precision
    {
        body = format!("{}{}", "0".repeat(precision - body.len()), body);
    }
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    if flags.alternate {
        match conv {
            b'o' => prefix.push_str("0o"),
            b'x' => prefix.push_str("0x"),
            b'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    Some(percent_apply_numeric_width(
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        flags.zero_pad && !flags.left_adjust,
    ))
}

fn percent_format_float(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_float_from_obj(_py, value_bits)?;
    let sign = if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    };
    let align = if flags.left_adjust {
        Some('<')
    } else if flags.zero_pad {
        Some('=')
    } else {
        None
    };
    let spec = FormatSpec {
        fill: if flags.zero_pad && !flags.left_adjust {
            '0'
        } else {
            ' '
        },
        align,
        sign,
        alternate: flags.alternate,
        width,
        grouping: None,
        precision,
        ty: Some(conv as char),
    };
    match format_float_with_spec(MoltObject::from_float(value), &spec) {
        Ok(text) => Some(text),
        Err((kind, msg)) => raise_exception::<Option<String>>(_py, kind, msg.as_ref()),
    }
}

fn percent_format_ascii(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<String> {
    let rendered_bits = molt_ascii_from_obj(value_bits);
    if exception_pending(_py) {
        if obj_from_bits(rendered_bits).as_ptr().is_some() {
            dec_ref_bits(_py, rendered_bits);
        }
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    if obj_from_bits(rendered_bits).as_ptr().is_some() {
        dec_ref_bits(_py, rendered_bits);
    }
    let rendered = rendered.unwrap_or_default();
    Some(percent_format_text(rendered, width, precision, flags))
}

fn percent_format_char(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<String> {
    let ch = percent_char_from_obj(_py, value_bits)?;
    Some(percent_apply_width(
        ch.to_string(),
        width,
        flags.left_adjust,
        ' ',
    ))
}

fn percent_lookup_mapping_arg(_py: &PyToken<'_>, rhs_bits: u64, key: &str) -> Option<(u64, bool)> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let Some(rhs_ptr) = rhs_obj.as_ptr() else {
        return raise_exception::<Option<(u64, bool)>>(
            _py,
            "TypeError",
            "format requires a mapping",
        );
    };
    unsafe {
        let rhs_type = object_type_id(rhs_ptr);
        if rhs_type == TYPE_ID_TUPLE {
            return raise_exception::<Option<(u64, bool)>>(
                _py,
                "TypeError",
                "format requires a mapping",
            );
        }
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if rhs_type == TYPE_ID_DICT {
            if let Some(bits) = dict_get_in_place(_py, rhs_ptr, key_bits) {
                dec_ref_bits(_py, key_bits);
                return Some((bits, false));
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, key_bits);
                return None;
            }
            raise_key_error_with_key::<()>(_py, key_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        if !percent_object_has_getitem(_py, rhs_ptr) {
            dec_ref_bits(_py, key_bits);
            return raise_exception::<Option<(u64, bool)>>(
                _py,
                "TypeError",
                "format requires a mapping",
            );
        }
        let bits = molt_index(rhs_bits, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return None;
        }
        Some((bits, true))
    }
}

fn percent_consume_next_arg(
    _py: &PyToken<'_>,
    rhs_bits: u64,
    tuple_ptr: Option<*mut u8>,
    tuple_idx: &mut usize,
    single_consumed: &mut bool,
) -> Option<u64> {
    if let Some(ptr) = tuple_ptr {
        let elems = unsafe { seq_vec_ref(ptr) };
        if *tuple_idx >= elems.len() {
            return raise_exception::<Option<u64>>(
                _py,
                "TypeError",
                "not enough arguments for format string",
            );
        }
        let bits = elems[*tuple_idx];
        *tuple_idx += 1;
        return Some(bits);
    }
    if *single_consumed {
        return raise_exception::<Option<u64>>(
            _py,
            "TypeError",
            "not enough arguments for format string",
        );
    }
    *single_consumed = true;
    Some(rhs_bits)
}

fn string_percent_format_impl(_py: &PyToken<'_>, text: &str, rhs_bits: u64) -> Option<String> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let tuple_ptr = rhs_obj
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_TUPLE });
    let mut tuple_idx = 0usize;
    let mut single_consumed = false;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut literal_start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }
        out.push_str(&text[literal_start..idx]);
        idx += 1;
        if idx >= bytes.len() {
            return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
        }
        if bytes[idx] == b'%' {
            out.push('%');
            idx += 1;
            literal_start = idx;
            continue;
        }
        let mut key: Option<&str> = None;
        if bytes[idx] == b'(' {
            let key_start = idx + 1;
            let mut key_end = key_start;
            while key_end < bytes.len() && bytes[key_end] != b')' {
                key_end += 1;
            }
            if key_end >= bytes.len() {
                return raise_exception::<Option<String>>(
                    _py,
                    "ValueError",
                    "incomplete format key",
                );
            }
            key = Some(&text[key_start..key_end]);
            idx = key_end + 1;
        }
        let mut flags = PercentFormatFlags::default();
        loop {
            if idx >= bytes.len() {
                return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
            }
            match bytes[idx] {
                b'-' => flags.left_adjust = true,
                b'+' => flags.sign_plus = true,
                b' ' => flags.sign_space = true,
                b'0' => flags.zero_pad = true,
                b'#' => flags.alternate = true,
                _ => break,
            }
            idx += 1;
        }
        let mut width = if idx < bytes.len() && bytes[idx].is_ascii_digit() {
            percent_parse_usize(_py, bytes, &mut idx, "width")
        } else {
            None
        };
        if idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
            let width_bits = percent_consume_next_arg(
                _py,
                rhs_bits,
                tuple_ptr,
                &mut tuple_idx,
                &mut single_consumed,
            )?;
            let width_val = index_i64_from_obj(_py, width_bits, "* wants int");
            if exception_pending(_py) {
                return None;
            }
            if width_val < 0 {
                flags.left_adjust = true;
                let abs = width_val.checked_abs().unwrap_or(i64::MAX);
                let Ok(width_usize) = usize::try_from(abs) else {
                    return raise_exception::<Option<String>>(
                        _py,
                        "OverflowError",
                        "width too big",
                    );
                };
                width = Some(width_usize);
            } else {
                let Ok(width_usize) = usize::try_from(width_val) else {
                    return raise_exception::<Option<String>>(
                        _py,
                        "OverflowError",
                        "width too big",
                    );
                };
                width = Some(width_usize);
            }
        }
        let mut precision: Option<usize> = None;
        if idx < bytes.len() && bytes[idx] == b'.' {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'*' {
                idx += 1;
                let prec_bits = percent_consume_next_arg(
                    _py,
                    rhs_bits,
                    tuple_ptr,
                    &mut tuple_idx,
                    &mut single_consumed,
                )?;
                let prec_val = index_i64_from_obj(_py, prec_bits, "* wants int");
                if exception_pending(_py) {
                    return None;
                }
                if prec_val <= 0 {
                    precision = Some(0);
                } else {
                    let Ok(prec_usize) = usize::try_from(prec_val) else {
                        return raise_exception::<Option<String>>(
                            _py,
                            "OverflowError",
                            "precision too big",
                        );
                    };
                    precision = Some(prec_usize);
                }
            } else {
                precision =
                    Some(percent_parse_usize(_py, bytes, &mut idx, "precision").unwrap_or(0));
            }
        }
        if idx < bytes.len() && (bytes[idx] == b'h' || bytes[idx] == b'l' || bytes[idx] == b'L') {
            let first = bytes[idx];
            idx += 1;
            if idx < bytes.len() && (first == b'h' || first == b'l') && bytes[idx] == first {
                idx += 1;
            }
        }
        if idx >= bytes.len() {
            return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
        }
        let conv_idx = idx;
        let conv = bytes[idx];
        idx += 1;
        let (value_bits, drop_value) = if let Some(key) = key {
            percent_lookup_mapping_arg(_py, rhs_bits, key)?
        } else {
            (
                percent_consume_next_arg(
                    _py,
                    rhs_bits,
                    tuple_ptr,
                    &mut tuple_idx,
                    &mut single_consumed,
                )?,
                false,
            )
        };
        let rendered = match conv {
            b's' => Some(percent_format_text(
                format_obj_str(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            )),
            b'r' => Some(percent_format_text(
                format_obj(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            )),
            b'a' => percent_format_ascii(_py, value_bits, width, precision, flags),
            b'c' => percent_format_char(_py, value_bits, width, flags),
            b'd' | b'i' | b'u' => {
                percent_format_decimal(_py, value_bits, width, precision, flags, conv)
            }
            b'o' | b'x' | b'X' => {
                percent_format_radix(_py, value_bits, width, precision, flags, conv)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                percent_format_float(_py, value_bits, width, precision, flags, conv)
            }
            _ => percent_unsupported_char(_py, conv, conv_idx),
        };
        if drop_value {
            dec_ref_bits(_py, value_bits);
        }
        let rendered = rendered?;
        out.push_str(&rendered);
        literal_start = idx;
    }
    out.push_str(&text[literal_start..]);
    if let Some(ptr) = tuple_ptr {
        let elems = unsafe { seq_vec_ref(ptr) };
        if tuple_idx < elems.len() {
            return raise_exception::<Option<String>>(
                _py,
                "TypeError",
                "not all arguments converted during string formatting",
            );
        }
    } else if !single_consumed && !percent_rhs_allows_unused_non_tuple(_py, rhs_obj) {
        return raise_exception::<Option<String>>(
            _py,
            "TypeError",
            "not all arguments converted during string formatting",
        );
    }
    Some(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        binary_type_error(_py, lhs, rhs, "%")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let imod_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.imod_name, b"__imod__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, imod_name_bits) {
                return res_bits;
            }
        }
        molt_mod(a, b)
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
    crate::with_gil_entry!(_py, {
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
        binary_type_error(_py, lhs, rhs, "**")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_pow(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let ipow_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ipow_name, b"__ipow__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ipow_name_bits) {
                return res_bits;
            }
        }
        molt_pow(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pow_mod(a: u64, b: u64, m: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
                return MoltObject::from_int(i).bits();
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                return MoltObject::from_int(i).bits();
            }
            let Some(ndigits) = to_i64(ndigits_obj) else {
                return raise_exception::<_>(_py, "TypeError", "round() ndigits must be int");
            };
            if ndigits >= 0 {
                return MoltObject::from_int(i).bits();
            }
            let exp = (-ndigits) as u32;
            if exp > 38 {
                return MoltObject::from_int(0).bits();
            }
            let pow = 10_i128.pow(exp);
            let value = i as i128;
            if pow == 0 {
                return MoltObject::from_int(i).bits();
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
    crate::with_gil_entry!(_py, {
        let val = obj_from_bits(val_bits);
        if let Some(i) = to_i64(val) {
            return MoltObject::from_int(i).bits();
        }
        if bigint_ptr_from_bits(val_bits).is_some() {
            return val_bits;
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
            set_add_in_place(_py, res_ptr, entry);
        }
        for &entry in r_elems.iter() {
            set_add_in_place(_py, res_ptr, entry);
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
        let (probe_elems, probe_table, output) = if l_elems.len() <= r_elems.len() {
            (r_elems, set_table(rhs_ptr), l_elems)
        } else {
            (l_elems, set_table(lhs_ptr), r_elems)
        };
        let res_bits = set_like_new_bits(result_type_id, output.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in output.iter() {
            let found = set_find_entry(_py, probe_elems, probe_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_some() {
                set_add_in_place(_py, res_ptr, entry);
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
        let r_table = set_table(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            let found = set_find_entry(_py, r_elems, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry);
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
            let found = set_find_entry(_py, r_elems, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        for &entry in r_elems.iter() {
            let found = set_find_entry(_py, l_elems, l_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry);
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
            set_add_in_place(_py, res_ptr, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_ptr_from_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
) -> Option<(*mut u8, Option<u64>)> {
    unsafe {
        let obj = obj_from_bits(other_bits);
        if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET {
                return Some((ptr, None));
            }
        }
        let set_bits = set_from_iter_bits(_py, other_bits)?;
        let ptr = obj_from_bits(set_bits).as_ptr()?;
        Some((ptr, Some(set_bits)))
    }
}

pub(super) unsafe fn set_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
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
            set_add_in_place(_py, set_ptr, val_bits);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
        let msg = format!("bad operand type for unary ~: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_neg(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
pub extern "C" fn molt_inplace_bit_xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let shift = index_i64_from_obj(_py, b, "shift count must be int");
        if shift < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative shift count");
        }
        let shift_u = shift as u32;
        if let Some(value) = to_i64(lhs) {
            if shift_u >= 63 {
                return bigint_bits(_py, BigInt::from(value) << shift_u);
            }
            let res = value << shift_u;
            if inline_int_from_i128(res as i128).is_some() {
                return int_bits_from_i64(_py, res);
            }
            return bigint_bits(_py, BigInt::from(value) << shift_u);
        }
        if let Some(value) = to_bigint(lhs) {
            let res = value << shift_u;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        binary_type_error(_py, lhs, rhs, "<<")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_lshift(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let shift = index_i64_from_obj(_py, b, "shift count must be int");
        if shift < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative shift count");
        }
        let shift_u = shift as u32;
        if let Some(value) = to_i64(lhs) {
            let res = if shift_u >= 63 {
                if value >= 0 { 0 } else { -1 }
            } else {
                value >> shift_u
            };
            return int_bits_from_i64(_py, res);
        }
        if let Some(value) = to_bigint(lhs) {
            let res = value >> shift_u;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        binary_type_error(_py, lhs, rhs, ">>")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_rshift(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_matmul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        binary_type_error(_py, lhs, rhs, "@")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_matmul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_matmul(a, b)
    })
}
