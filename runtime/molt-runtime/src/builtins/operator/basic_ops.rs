use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_index(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(obj_bits))
        );
        let Some(value) = index_bigint_from_obj(_py, obj_bits, &err) else {
            return MoltObject::none().bits();
        };
        int_bits_from_bigint(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_abs(val: u64) -> u64 {
    molt_abs_builtin(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_add(a: u64, b: u64) -> u64 {
    molt_add(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_sub(a: u64, b: u64) -> u64 {
    molt_sub(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_mul(a: u64, b: u64) -> u64 {
    molt_mul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_matmul(a: u64, b: u64) -> u64 {
    molt_matmul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_truediv(a: u64, b: u64) -> u64 {
    molt_div(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_floordiv(a: u64, b: u64) -> u64 {
    molt_floordiv(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_mod(a: u64, b: u64) -> u64 {
    molt_mod(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_pow(a: u64, b: u64) -> u64 {
    molt_pow(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_lshift(a: u64, b: u64) -> u64 {
    molt_lshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_rshift(a: u64, b: u64) -> u64 {
    molt_rshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_and(a: u64, b: u64) -> u64 {
    molt_bit_and(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_or(a: u64, b: u64) -> u64 {
    molt_bit_or(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_xor(a: u64, b: u64) -> u64 {
    molt_bit_xor(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_neg(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            return int_bits_from_i128(_py, -(i as i128));
        }
        if let Some(big) = to_bigint(obj) {
            let res = -big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some(f) = crate::object::ops::as_float_extended(obj) {
            return crate::object::ops::float_result_bits(_py, -f);
        }
        if complex_ptr_from_bits(val).is_some() {
            match complex_from_obj_strict(_py, obj) {
                Ok(Some(c)) => return complex_bits(_py, -c.re, -c.im),
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        if let Some(ptr) = obj.as_ptr() {
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__neg__") else {
                return MoltObject::none().bits();
            };
            let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
            dec_ref_bits(_py, name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = unsafe { call_callable0(_py, call_bits) };
                dec_ref_bits(_py, call_bits);
                return res_bits;
            }
        }
        let msg = format!("bad operand type for unary -: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_pos(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            // Full-range boxing — `from_int` would silently truncate a fit-i64
            // BigInt (e.g. operator.pos(2**60)) or exact-integer float >= 2**46.
            return int_bits_from_i64(_py, i);
        }
        if let Some(big) = to_bigint(obj) {
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, big);
        }
        if let Some(f) = crate::object::ops::as_float_extended(obj) {
            return crate::object::ops::float_result_bits(_py, f);
        }
        if complex_ptr_from_bits(val).is_some() {
            match complex_from_obj_strict(_py, obj) {
                Ok(Some(c)) => return complex_bits(_py, c.re, c.im),
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        if let Some(ptr) = obj.as_ptr() {
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__pos__") else {
                return MoltObject::none().bits();
            };
            let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
            dec_ref_bits(_py, name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = unsafe { call_callable0(_py, call_bits) };
                dec_ref_bits(_py, call_bits);
                return res_bits;
            }
        }
        let msg = format!("bad operand type for unary +: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_invert(val: u64) -> u64 {
    molt_invert(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_not(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let truthy = molt_is_truthy(val) != 0;
        MoltObject::from_bool(!truthy).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_truth(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let truthy = molt_is_truthy(val) != 0;
        MoltObject::from_bool(truthy).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_eq(a: u64, b: u64) -> u64 {
    molt_eq(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ne(a: u64, b: u64) -> u64 {
    molt_ne(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_lt(a: u64, b: u64) -> u64 {
    molt_lt(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_le(a: u64, b: u64) -> u64 {
    molt_le(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_gt(a: u64, b: u64) -> u64 {
    molt_gt(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ge(a: u64, b: u64) -> u64 {
    molt_ge(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_is(a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a == b).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_is_not(a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a != b).bits()
}
