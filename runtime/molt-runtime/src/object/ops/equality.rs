use super::*;

pub(in crate::object) unsafe fn eq_bool_from_bits(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
) -> Option<bool> {
    let pending_before = exception_pending(_py);
    let prev_exc_bits = if pending_before {
        exception_last_bits_noinc(_py).unwrap_or(0)
    } else {
        0
    };
    let res_bits = molt_eq(lhs_bits, rhs_bits);
    if exception_pending(_py) {
        if !pending_before {
            return None;
        }
        let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
        if after_exc_bits != prev_exc_bits {
            return None;
        }
    }
    let res_obj = obj_from_bits(res_bits);
    if pending_before && res_obj.is_none() {
        return Some(obj_eq(
            _py,
            obj_from_bits(lhs_bits),
            obj_from_bits(rhs_bits),
        ));
    }
    Some(is_truthy(_py, res_obj))
}

pub(in crate::object) enum BinaryDunderOutcome {
    Value(u64),
    NotImplemented,
    Missing,
    Error,
}

pub(in crate::object) unsafe fn call_dunder_raw(
    _py: &PyToken<'_>,
    raw_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    arg_bits: u64,
) -> BinaryDunderOutcome {
    unsafe {
        let Some(inst_ptr) = instance_ptr else {
            return BinaryDunderOutcome::Missing;
        };
        let Some(bound_bits) = descriptor_bind(_py, raw_bits, owner_ptr, Some(inst_ptr)) else {
            if exception_pending(_py) {
                return BinaryDunderOutcome::Error;
            }
            return BinaryDunderOutcome::Missing;
        };
        let res_bits = call_callable1(_py, bound_bits, arg_bits);
        dec_ref_bits(_py, bound_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, res_bits);
            return BinaryDunderOutcome::Error;
        }
        if is_not_implemented_bits(_py, res_bits) {
            dec_ref_bits(_py, res_bits);
            return BinaryDunderOutcome::NotImplemented;
        }
        BinaryDunderOutcome::Value(res_bits)
    }
}

pub(in crate::object) unsafe fn call_binary_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
) -> Option<u64> {
    unsafe {
        let lhs_obj = obj_from_bits(lhs_bits);
        let rhs_obj = obj_from_bits(rhs_bits);
        let lhs_ptr = lhs_obj.as_ptr();
        let rhs_ptr = rhs_obj.as_ptr();

        let lhs_type_bits = type_of_bits(_py, lhs_bits);
        let rhs_type_bits = type_of_bits(_py, rhs_bits);
        let lhs_type_ptr = obj_from_bits(lhs_type_bits).as_ptr();
        let rhs_type_ptr = obj_from_bits(rhs_type_bits).as_ptr();

        let lhs_op_raw =
            lhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, op_name_bits));
        let rhs_rop_raw =
            rhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, rop_name_bits));

        let rhs_is_subclass =
            rhs_type_bits != lhs_type_bits && issubclass_bits(rhs_type_bits, lhs_type_bits);
        let prefer_rhs = rhs_is_subclass
            && rhs_rop_raw.is_some()
            && lhs_op_raw.is_none_or(|lhs_raw| lhs_raw != rhs_rop_raw.unwrap());

        let mut tried_rhs = false;
        if prefer_rhs
            && let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
                (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            tried_rhs = true;
            match call_dunder_raw(_py, rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        if let (Some(lhs_ptr), Some(lhs_type_ptr), Some(lhs_raw)) =
            (lhs_ptr, lhs_type_ptr, lhs_op_raw)
        {
            match call_dunder_raw(_py, lhs_raw, lhs_type_ptr, Some(lhs_ptr), rhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        if !tried_rhs
            && let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
                (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            match call_dunder_raw(_py, rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
        None
    }
}

pub(in crate::object) unsafe fn call_inplace_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
) -> Option<u64> {
    unsafe {
        if let Some(lhs_ptr) = obj_from_bits(lhs_bits).as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr(_py, lhs_ptr, op_name_bits) {
                let res_bits = call_callable1(_py, call_bits, rhs_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return Some(MoltObject::none().bits());
                }
                if !is_not_implemented_bits(_py, res_bits) {
                    return Some(res_bits);
                }
                dec_ref_bits(_py, res_bits);
            }
            if exception_pending(_py) {
                return Some(MoltObject::none().bits());
            }
        }
        None
    }
}

pub(crate) fn obj_eq(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> bool {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return li == ri;
    }
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    if (is_float_extended(lhs) || is_float_extended(rhs))
        && let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs))
    {
        return lf == rf;
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return l_big == r_big;
    }
    if complex_ptr_from_bits(lhs.bits()).is_some() || complex_ptr_from_bits(rhs.bits()).is_some() {
        let l_complex = complex_from_obj_lossy(lhs);
        let r_complex = complex_from_obj_lossy(rhs);
        if let (Some(lc), Some(rc)) = (l_complex, r_complex) {
            return lc.re == rc.re && lc.im == rc.im;
        }
        return false;
    }
    if let (Some(lp), Some(rp)) = (
        maybe_ptr_from_bits(lhs.bits()),
        maybe_ptr_from_bits(rhs.bits()),
    ) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if ltype != rtype {
                if (ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTEARRAY)
                    || (ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTES)
                {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    if l_len != r_len {
                        return false;
                    }
                    return simd_bytes_eq(bytes_data(lp), bytes_data(rp), l_len);
                }
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    let l_elems = set_order(lp);
                    let r_elems = set_order(rp);
                    if l_elems.len() != r_elems.len() {
                        return false;
                    }
                    let r_table = set_table(rp);
                    let r_hashes = set_hashes(rp);
                    for key_bits in l_elems.iter().copied() {
                        if set_find_entry_fast(_py, r_elems, r_hashes, r_table, key_bits).is_none()
                        {
                            return false;
                        }
                    }
                    return true;
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return false;
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return false;
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
                            return false;
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return false;
                        };
                        (ptr, Some(bits))
                    };
                    let l_elems = set_order(lhs_ptr);
                    let r_elems = set_order(rhs_ptr);
                    let mut equal = true;
                    if l_elems.len() != r_elems.len() {
                        equal = false;
                    } else {
                        let r_table = set_table(rhs_ptr);
                        let r_hashes = set_hashes(rhs_ptr);
                        for key_bits in l_elems.iter().copied() {
                            if set_find_entry_fast(_py, r_elems, r_hashes, r_table, key_bits)
                                .is_none()
                            {
                                equal = false;
                                break;
                            }
                        }
                    }
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return equal;
                }
                return false;
            }
            if ltype == TYPE_ID_STRING {
                let l_len = string_len(lp);
                let r_len = string_len(rp);
                if l_len != r_len {
                    return false;
                }
                return simd_bytes_eq(string_bytes(lp), string_bytes(rp), l_len);
            }
            if ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                if l_len != r_len {
                    return false;
                }
                return simd_bytes_eq(bytes_data(lp), bytes_data(rp), l_len);
            }
            if ltype == TYPE_ID_TUPLE {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                // SIMD fast path: skip past identity-equal prefix
                let first_diff = simd_find_first_mismatch(l_elems, r_elems);
                for idx in first_diff..l_elems.len() {
                    if !obj_eq(
                        _py,
                        obj_from_bits(l_elems[idx]),
                        obj_from_bits(r_elems[idx]),
                    ) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_SLICE {
                let l_start = slice_start_bits(lp);
                let l_stop = slice_stop_bits(lp);
                let l_step = slice_step_bits(lp);
                let r_start = slice_start_bits(rp);
                let r_stop = slice_stop_bits(rp);
                let r_step = slice_step_bits(rp);
                if !obj_eq(_py, obj_from_bits(l_start), obj_from_bits(r_start)) {
                    return false;
                }
                if !obj_eq(_py, obj_from_bits(l_stop), obj_from_bits(r_stop)) {
                    return false;
                }
                if !obj_eq(_py, obj_from_bits(l_step), obj_from_bits(r_step)) {
                    return false;
                }
                return true;
            }
            if ltype == TYPE_ID_GENERIC_ALIAS {
                let l_origin = generic_alias_origin_bits(lp);
                let l_args = generic_alias_args_bits(lp);
                let r_origin = generic_alias_origin_bits(rp);
                let r_args = generic_alias_args_bits(rp);
                return obj_eq(_py, obj_from_bits(l_origin), obj_from_bits(r_origin))
                    && obj_eq(_py, obj_from_bits(l_args), obj_from_bits(r_args));
            }
            if ltype == TYPE_ID_UNION {
                let l_args = union_type_args_bits(lp);
                let r_args = union_type_args_bits(rp);
                return obj_eq(_py, obj_from_bits(l_args), obj_from_bits(r_args));
            }
            // Identity check: if pointers are equal, the objects are equal
            // (handles self-referential containers without infinite recursion).
            if lp == rp {
                return true;
            }
            if ltype == TYPE_ID_LIST {
                // Recursion guard for nested/self-referential containers
                if !crate::state::recursion::recursion_guard_enter_fast() {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded in comparison",
                    );
                    return false;
                }
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    crate::state::recursion::recursion_guard_exit_fast();
                    return false;
                }
                // SIMD fast path: skip past identity-equal prefix
                let first_diff = simd_find_first_mismatch(l_elems, r_elems);
                for idx in first_diff..l_elems.len() {
                    if !obj_eq(
                        _py,
                        obj_from_bits(l_elems[idx]),
                        obj_from_bits(r_elems[idx]),
                    ) {
                        crate::state::recursion::recursion_guard_exit_fast();
                        return false;
                    }
                }
                crate::state::recursion::recursion_guard_exit_fast();
                return true;
            }
            if ltype == TYPE_ID_DICT {
                if !crate::state::recursion::recursion_guard_enter_fast() {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded in comparison",
                    );
                    return false;
                }
                let l_pairs = dict_order(lp);
                let r_pairs = dict_order(rp);
                if l_pairs.len() != r_pairs.len() {
                    crate::state::recursion::recursion_guard_exit_fast();
                    return false;
                }
                let r_table = dict_table(rp);
                let r_hashes = dict_hashes(rp);
                let entries = l_pairs.len() / 2;
                for entry_idx in 0..entries {
                    let key_bits = l_pairs[entry_idx * 2];
                    let val_bits = l_pairs[entry_idx * 2 + 1];
                    let Some(r_entry_idx) =
                        dict_find_entry_fast(_py, r_pairs, r_hashes, r_table, key_bits)
                    else {
                        crate::state::recursion::recursion_guard_exit_fast();
                        return false;
                    };
                    let r_val_bits = r_pairs[r_entry_idx * 2 + 1];
                    if !obj_eq(_py, obj_from_bits(val_bits), obj_from_bits(r_val_bits)) {
                        crate::state::recursion::recursion_guard_exit_fast();
                        return false;
                    }
                }
                crate::state::recursion::recursion_guard_exit_fast();
                return true;
            }
            if ltype == TYPE_ID_SET || ltype == TYPE_ID_FROZENSET {
                let l_elems = set_order(lp);
                let r_elems = set_order(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                let r_table = set_table(rp);
                let r_hashes = set_hashes(rp);
                for key_bits in l_elems.iter().copied() {
                    if set_find_entry_fast(_py, r_elems, r_hashes, r_table, key_bits).is_none() {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_DATACLASS {
                let l_desc = dataclass_desc_ptr(lp);
                let r_desc = dataclass_desc_ptr(rp);
                if l_desc.is_null() || r_desc.is_null() {
                    return false;
                }
                let l_desc = &*l_desc;
                let r_desc = &*r_desc;
                if !l_desc.eq || !r_desc.eq {
                    return lp == rp;
                }
                if l_desc.name != r_desc.name || l_desc.field_names != r_desc.field_names {
                    return false;
                }
                let l_vals = dataclass_fields_ref(lp);
                let r_vals = dataclass_fields_ref(rp);
                if l_vals.len() != r_vals.len() {
                    return false;
                }
                for (idx, (l_val, r_val)) in l_vals.iter().zip(r_vals.iter()).enumerate() {
                    let flag = l_desc.field_flags.get(idx).copied().unwrap_or(0x7);
                    if (flag & 0x2) == 0 {
                        continue;
                    }
                    if is_missing_bits(_py, *l_val) || is_missing_bits(_py, *r_val) {
                        return false;
                    }
                    if !obj_eq(_py, obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
            // Function equality: two functions with the same code object
            // are equal (CPython parity: len == len is True).
            if ltype == TYPE_ID_FUNCTION {
                let l_code = function_code_bits(lp);
                let r_code = function_code_bits(rp);
                if l_code != 0 && l_code == r_code {
                    return true;
                }
            }
            // Range equality: range(a,b,c) == range(a,b,c) if start,stop,step match.
            if ltype == TYPE_ID_RANGE {
                return obj_eq(
                    _py,
                    obj_from_bits(range_start_bits(lp)),
                    obj_from_bits(range_start_bits(rp)),
                ) && obj_eq(
                    _py,
                    obj_from_bits(range_stop_bits(lp)),
                    obj_from_bits(range_stop_bits(rp)),
                ) && obj_eq(
                    _py,
                    obj_from_bits(range_step_bits(lp)),
                    obj_from_bits(range_step_bits(rp)),
                );
            }
        }
        return lp == rp;
    }
    false
}
