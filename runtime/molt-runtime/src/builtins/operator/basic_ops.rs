use molt_obj_model::MoltObject;

use crate::builtins::numbers::{exact_integer_from_owned, index_from_object};
use crate::{
    molt_abs_builtin_attached_body, molt_add_attached_body, molt_bit_and_attached_body,
    molt_bit_or_attached_body, molt_bit_xor_attached_body, molt_div_attached_body,
    molt_eq_attached_body, molt_floordiv_attached_body, molt_ge_attached_body,
    molt_gt_attached_body, molt_invert_attached_body, molt_is_truthy_attached_body,
    molt_le_attached_body, molt_lshift_attached_body, molt_lt_attached_body,
    molt_matmul_attached_body, molt_mod_attached_body, molt_mul_attached_body,
    molt_ne_attached_body, molt_pow_attached_body, molt_rshift_attached_body,
    molt_sub_attached_body,
};

pub(crate) fn molt_operator_index_attached_body(_py: &crate::PyToken<'_>, obj_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if crate::exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let value = index_from_object(_py, obj_bits);
        if crate::exception_pending(_py) {
            if MoltObject::from_bits(value).is_ptr() {
                drop(crate::builtins::exceptions::ExceptionValue::adopt(
                    _py, value,
                ));
            }
            return MoltObject::none().bits();
        }
        exact_integer_from_owned(_py, value)
    })
}

pub(crate) fn molt_operator_abs_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    molt_abs_builtin_attached_body(_py, val)
}

pub(crate) fn molt_operator_add_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_add_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_sub_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_sub_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_mul_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_mul_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_matmul_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_matmul_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_truediv_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_div_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_floordiv_attached_body(
    _py: &crate::PyToken<'_>,
    a: u64,
    b: u64,
) -> u64 {
    molt_floordiv_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_mod_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_mod_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_pow_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_pow_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_lshift_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_lshift_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_rshift_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_rshift_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_and_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_bit_and_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_or_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_bit_or_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_xor_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_bit_xor_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_neg_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    crate::molt_neg_attached_body(_py, val)
}

pub(crate) fn molt_operator_pos_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    crate::molt_pos_attached_body(_py, val)
}

pub(crate) fn molt_operator_invert_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    molt_invert_attached_body(_py, val)
}

pub(crate) fn molt_operator_not_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let truthy = molt_is_truthy_attached_body(_py, val) != 0;
        MoltObject::from_bool(!truthy).bits()
    })
}

pub(crate) fn molt_operator_truth_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let truthy = molt_is_truthy_attached_body(_py, val) != 0;
        MoltObject::from_bool(truthy).bits()
    })
}

pub(crate) fn molt_operator_eq_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_eq_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ne_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_ne_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_lt_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_lt_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_le_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_le_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_gt_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_gt_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ge_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_ge_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_is_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a == b).bits()
}

pub(crate) fn molt_operator_is_not_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a != b).bits()
}

include!("../../attached_exports_generated/builtins/operator/basic_ops.rs");
