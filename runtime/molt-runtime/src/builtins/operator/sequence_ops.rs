use molt_obj_model::MoltObject;
use num_traits::{Signed, ToPrimitive};

use crate::builtins::exceptions::{exception_matches_builtin_name, molt_exception_last_pending};
use crate::builtins::numbers::index_bigint_from_obj;
use crate::{
    TYPE_ID_TUPLE, attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, bigint_ptr_from_bits,
    bigint_ref, call_callable0, class_name_for_error, dec_ref_bits, exception_pending,
    exception_stack_pop, exception_stack_push, is_truthy, molt_concat, molt_contains,
    molt_delitem_method, molt_eq, molt_getitem_method, molt_inplace_add, molt_inplace_bit_and,
    molt_inplace_bit_or, molt_inplace_bit_xor, molt_inplace_concat, molt_inplace_div,
    molt_inplace_floordiv, molt_inplace_lshift, molt_inplace_matmul, molt_inplace_mod,
    molt_inplace_mul, molt_inplace_pow, molt_inplace_rshift, molt_inplace_sub, molt_iter_checked,
    molt_iter_next, molt_len, molt_setitem_method, obj_from_bits, object_type_id, raise_exception,
    seq_vec_ref, to_i64, type_name, type_of_bits,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_contains(container_bits: u64, item_bits: u64) -> u64 {
    molt_contains(container_bits, item_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_getitem(obj_bits: u64, key_bits: u64) -> u64 {
    molt_getitem_method(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_setitem(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    molt_setitem_method(obj_bits, key_bits, val_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_delitem(obj_bits: u64, key_bits: u64) -> u64 {
    molt_delitem_method(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_countof(container_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let iter_bits = molt_iter_checked(container_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mut count: i64 = 0;
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(pair_ptr) = obj_from_bits(pair_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let eq_bits = molt_eq(val_bits, value_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if is_truthy(_py, obj_from_bits(eq_bits)) {
                    count += 1;
                }
            }
        }
        MoltObject::from_int(count).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_length_hint(obj_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let default_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(default_bits))
        );
        let Some(default_value) = index_bigint_from_obj(_py, default_bits, &default_err) else {
            return MoltObject::none().bits();
        };
        let Some(default_value) = default_value.to_i64() else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            );
        };
        let default_bits = MoltObject::from_int(default_value).bits();
        exception_stack_push();
        let len_bits = molt_len(obj_bits);
        if !exception_pending(_py) {
            exception_stack_pop(_py);
            return len_bits;
        }
        let exc_bits = molt_exception_last_pending();
        if !exception_matches_builtin_name(_py, exc_bits, "TypeError") {
            exception_stack_pop(_py);
            dec_ref_bits(_py, exc_bits);
            return MoltObject::none().bits();
        }
        crate::molt_exception_clear();
        dec_ref_bits(_py, exc_bits);
        exception_stack_pop(_py);

        if let Some(ptr) = obj.as_ptr() {
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__length_hint__") else {
                return MoltObject::none().bits();
            };
            let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
            dec_ref_bits(_py, name_bits);
            if let Some(call_bits) = call_bits {
                exception_stack_push();
                let res_bits = unsafe { call_callable0(_py, call_bits) };
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last_pending();
                    if exception_matches_builtin_name(_py, exc_bits, "TypeError") {
                        crate::molt_exception_clear();
                        dec_ref_bits(_py, exc_bits);
                        exception_stack_pop(_py);
                        return default_bits;
                    }
                    dec_ref_bits(_py, exc_bits);
                    exception_stack_pop(_py);
                    return MoltObject::none().bits();
                }
                exception_stack_pop(_py);
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if i < 0 {
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "__length_hint__() should return >= 0",
                        );
                    }
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return MoltObject::from_int(i).bits();
                }
                if let Some(ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = unsafe { bigint_ref(ptr) };
                    if big.is_negative() {
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "__length_hint__() should return >= 0",
                        );
                    }
                    let Some(len) = big.to_usize() else {
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(
                            _py,
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        );
                    };
                    if len > i64::MAX as usize {
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(
                            _py,
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        );
                    }
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::from_int(len as i64).bits();
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                let msg = format!("__length_hint__ must be an integer, not {res_type}");
                dec_ref_bits(_py, res_bits);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_concat(a: u64, b: u64) -> u64 {
    molt_concat(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iconcat(a: u64, b: u64) -> u64 {
    molt_inplace_concat(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iadd(a: u64, b: u64) -> u64 {
    molt_inplace_add(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_isub(a: u64, b: u64) -> u64 {
    molt_inplace_sub(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imul(a: u64, b: u64) -> u64 {
    molt_inplace_mul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imatmul(a: u64, b: u64) -> u64 {
    molt_inplace_matmul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itruediv(a: u64, b: u64) -> u64 {
    molt_inplace_div(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ifloordiv(a: u64, b: u64) -> u64 {
    molt_inplace_floordiv(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imod(a: u64, b: u64) -> u64 {
    molt_inplace_mod(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ipow(a: u64, b: u64) -> u64 {
    molt_inplace_pow(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ilshift(a: u64, b: u64) -> u64 {
    molt_inplace_lshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_irshift(a: u64, b: u64) -> u64 {
    molt_inplace_rshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iand(a: u64, b: u64) -> u64 {
    molt_inplace_bit_and(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ior(a: u64, b: u64) -> u64 {
    molt_inplace_bit_or(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ixor(a: u64, b: u64) -> u64 {
    molt_inplace_bit_xor(a, b)
}
