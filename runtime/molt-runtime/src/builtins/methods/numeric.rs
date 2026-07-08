use super::common::{
    builtin_classmethod_bits, builtin_classmethod_bits_with_defaults_tuple, builtin_func_bits,
    builtin_func_bits_with_defaults_tuple, runtime_python_at_least,
};
use crate::PyToken;
use crate::object::ops_hash::{molt_float_hash_method, molt_int_hash_method};
use crate::*;

pub(crate) fn int_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__abs__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_abs,
            fn_addr!(molt_int_abs_method),
            1,
        )),
        "__add__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_add,
            fn_addr!(molt_int_add_method),
            2,
        )),
        "__and__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_and,
            fn_addr!(molt_int_and_method),
            2,
        )),
        "__bool__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_bool,
            fn_addr!(molt_int_bool_method),
            1,
        )),
        "__ceil__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_ceil,
            fn_addr!(molt_int_ceil_method),
            1,
        )),
        "__divmod__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_divmod,
            fn_addr!(molt_int_divmod_method),
            2,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_new,
            fn_addr!(molt_int_new),
            3,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_hash,
            fn_addr!(molt_int_hash_method),
            1,
        )),
        "__int__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_int,
            fn_addr!(molt_int_int),
            1,
        )),
        "__index__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_index,
            fn_addr!(molt_int_index),
            1,
        )),
        "bit_length" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_bit_length,
            fn_addr!(molt_int_bit_length),
            1,
        )),
        "bit_count" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_bit_count,
            fn_addr!(molt_int_bit_count),
            1,
        )),
        "as_integer_ratio" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_as_integer_ratio,
            fn_addr!(molt_int_as_integer_ratio),
            1,
        )),
        "conjugate" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_conjugate,
            fn_addr!(molt_int_conjugate),
            1,
        )),
        "is_integer" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_is_integer,
            fn_addr!(molt_int_is_integer),
            1,
        )),
        "to_bytes" => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.int_to_bytes,
                fn_addr!(molt_int_to_bytes),
                4,
                &[zero],
            ))
        }
        _ => None,
    }
}

pub(crate) fn int_class_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "from_bytes" => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_classmethod_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.int_from_bytes,
                fn_addr!(molt_int_from_bytes),
                4,
                &[zero],
            ))
        }
        _ => None,
    }
}

pub(crate) fn float_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__new__" => {
            let zero = MoltObject::from_float(0.0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.float_new,
                fn_addr!(molt_float_new),
                2,
                &[zero],
            ))
        }
        "__hash__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.float_hash,
            fn_addr!(molt_float_hash_method),
            1,
        )),
        "__float__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.float_float,
            fn_addr!(molt_float_float),
            1,
        )),
        "as_integer_ratio" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.float_as_integer_ratio,
            fn_addr!(molt_float_as_integer_ratio),
            1,
        )),
        "conjugate" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.float_conjugate,
            fn_addr!(molt_float_conjugate),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.float_hex,
            fn_addr!(molt_float_hex),
            1,
        )),
        "is_integer" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.float_is_integer,
            fn_addr!(molt_float_is_integer),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn float_class_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "fromhex" => Some(builtin_classmethod_bits(
            _py,
            &runtime_state(_py).method_cache.float_fromhex,
            fn_addr!(molt_float_fromhex),
            2,
        )),
        "from_number" if runtime_python_at_least(_py, 3, 14) => Some(builtin_classmethod_bits(
            _py,
            &runtime_state(_py).method_cache.float_from_number,
            fn_addr!(molt_float_from_number),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn complex_class_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "from_number" if runtime_python_at_least(_py, 3, 14) => Some(builtin_classmethod_bits(
            _py,
            &runtime_state(_py).method_cache.complex_from_number,
            fn_addr!(molt_complex_from_number),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn complex_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "conjugate" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.complex_conjugate,
            fn_addr!(molt_complex_conjugate),
            1,
        )),
        _ => None,
    }
}
