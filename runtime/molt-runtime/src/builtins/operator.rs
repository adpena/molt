use std::sync::atomic::{AtomicU64, Ordering};

use molt_obj_model::MoltObject;
use num_traits::{Signed, ToPrimitive};

use crate::builtins::exceptions::{exception_matches_builtin_name, molt_exception_last_pending};
use crate::builtins::numbers::{index_bigint_from_obj, int_bits_from_bigint, int_bits_from_i64};
use crate::{
    PyToken, TYPE_ID_DICT, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_class_obj, alloc_dict_with_pairs,
    alloc_string, alloc_tuple, attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes,
    bigint_bits, bigint_ptr_from_bits, bigint_ref, bigint_to_inline, builtin_classes,
    call_callable0, class_dict_bits, class_name_for_error, complex_bits, complex_from_obj_strict,
    complex_ptr_from_bits, dec_ref_bits, dict_set_in_place, exception_pending, exception_stack_pop,
    exception_stack_push, inc_ref_bits, init_atomic_bits, int_bits_from_i128, intern_static_name,
    is_truthy, molt_abs_builtin, molt_add, molt_bit_and, molt_bit_or, molt_bit_xor,
    molt_class_set_base, molt_concat, molt_contains, molt_delitem_method, molt_div, molt_eq,
    molt_floordiv, molt_ge, molt_getattr_builtin, molt_getitem_method, molt_gt, molt_index,
    molt_inplace_add, molt_inplace_bit_and, molt_inplace_bit_or, molt_inplace_bit_xor,
    molt_inplace_concat, molt_inplace_div, molt_inplace_floordiv, molt_inplace_lshift,
    molt_inplace_matmul, molt_inplace_mod, molt_inplace_mul, molt_inplace_pow, molt_inplace_rshift,
    molt_inplace_sub, molt_invert, molt_is_truthy, molt_iter_checked, molt_iter_next, molt_le,
    molt_len, molt_lshift, molt_lt, molt_matmul, molt_mod, molt_mul, molt_ne, molt_pow,
    molt_rshift, molt_setitem_method, molt_sub, obj_from_bits, object_class_bits,
    object_set_class_bits, object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned,
    to_bigint, to_i64, type_name, type_of_bits,
};

mod basic_ops;
mod class_support;
mod getter_objects;
mod sequence_ops;
mod state;
#[cfg(test)]
mod tests;

pub use basic_ops::*;
pub use getter_objects::*;
pub use sequence_ops::*;
pub(crate) use state::{OperatorRuntimeState, operator_clear_runtime_state};
