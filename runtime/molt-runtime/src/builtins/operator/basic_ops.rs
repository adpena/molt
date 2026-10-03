use molt_obj_model::MoltObject;

use crate::builtins::numbers::{index_bigint_from_obj, int_bits_from_bigint};
use crate::{
    molt_abs_builtin, molt_add, molt_bit_and, molt_bit_or, molt_bit_xor, molt_div,
    molt_eq, molt_floordiv, molt_ge, molt_gt, molt_invert, molt_is_truthy, molt_le,
    molt_lshift, molt_lt, molt_matmul, molt_mod, molt_mul, molt_ne, molt_pow,
    molt_rshift, molt_sub, obj_from_bits, type_name,
};

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
    crate::molt_neg(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_pos(val: u64) -> u64 {
    crate::molt_pos(val)
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
